//! `AskUserQuestion`'s input and its answer, read the same way by both roads
//! that run it: the streaming turn (a terminal or window at the keyboard) and
//! the tools crate's dispatch (a pipe, a helper, a terminal with no screen).
//!
//! The schema is Claude Code's: a `questions` array of one to
//! [`MAX_QUESTIONS`], each with a `header` chip and [`MIN_OPTIONS`] to
//! [`MAX_OPTIONS`] options. The flat single question older calls and running
//! sessions send is still read, as a list of one, with header and options
//! optional as they were. The answer comes back per question in `answers`
//! (question → answer); a call of one question also keeps its historical
//! `question` / `answer` pair.

use serde_json::{json, Map, Value};

use super::types::QuestionOption;

/// At most this many questions in one call.
pub const MAX_QUESTIONS: usize = 4;
/// A question of the `questions` array offers at least this many choices…
pub const MIN_OPTIONS: usize = 2;
/// …and at most this many.
pub const MAX_OPTIONS: usize = 4;

/// One question of a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskedQuestion {
    /// The question as the model wrote it.
    pub question: String,
    /// The short chip naming the decision (`Auth method`).
    pub header: Option<String>,
    /// The fixed choices in display order; empty for a free-form question.
    pub options: Vec<QuestionOption>,
    /// Several options may be picked. Meaningless without options.
    pub multi_select: bool,
}

impl AskedQuestion {
    /// Several options may be picked: only a question that has options.
    #[must_use]
    pub fn is_multi(&self) -> bool {
        self.multi_select && !self.options.is_empty()
    }
}

/// Where one question stands in a call that asks several — what the
/// terminal's overlay draws its tabs and its last, gathering screen from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuestionStep {
    /// This question's place, from 0.
    pub index: usize,
    /// Every question's chip, in order (empty when a question has none).
    pub headers: Vec<String>,
    /// The questions before this one and what each was answered.
    pub answered: Vec<(String, Vec<String>)>,
}

impl QuestionStep {
    /// How many questions the call asks.
    #[must_use]
    pub fn total(&self) -> usize {
        self.headers.len()
    }

    /// This is the call's last question: answering it sends them all.
    #[must_use]
    pub fn is_last(&self) -> bool {
        self.index + 1 >= self.total()
    }
}

/// The questions of one `AskUserQuestion` input.
///
/// # Errors
/// A message the model can act on: a `questions` array that is empty, longer
/// than [`MAX_QUESTIONS`], or holds a question with no text, no `header`, or
/// a number of options outside [`MIN_OPTIONS`]..=[`MAX_OPTIONS`]; a flat
/// question with no text; options or `multiSelect` of the wrong type.
pub fn parse_questions(value: &Value) -> Result<Vec<AskedQuestion>, String> {
    let Some(list) = value.get("questions") else {
        return Ok(vec![parse_one(value)?]);
    };
    let list = list
        .as_array()
        .ok_or_else(|| "AskUserQuestion `questions` must be an array".to_string())?;
    if list.is_empty() || list.len() > MAX_QUESTIONS {
        return Err(format!(
            "AskUserQuestion takes 1 to {MAX_QUESTIONS} questions, not {}; ask the rest in a later call",
            list.len()
        ));
    }
    let mut questions = Vec::with_capacity(list.len());
    for (index, entry) in list.iter().enumerate() {
        let number = index + 1;
        let question = parse_one(entry).map_err(|error| format!("question {number}: {error}"))?;
        if question.header.as_deref().is_none_or(|header| header.trim().is_empty()) {
            return Err(format!(
                "question {number}: `header` is required — a chip of about 12 characters"
            ));
        }
        let count = question.options.len();
        if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&count) {
            return Err(format!(
                "question {number}: give {MIN_OPTIONS} to {MAX_OPTIONS} options, not {count}; the person can still type their own answer"
            ));
        }
        questions.push(question);
    }
    Ok(questions)
}

/// One question, from the several shapes models emit. Beyond the canonical
/// object it reads:
/// - `options` double-encoded as a JSON string,
/// - `multiSelect` (or `multi_select`) as the words `true` / `false`,
/// - the whole object JSON-encoded into the `question` string,
/// - prose followed by a JSON array of options dumped into `question`.
///
/// Fields given at the top win; recovered ones only fill gaps.
fn parse_one(value: &Value) -> Result<AskedQuestion, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "AskUserQuestion input must be a JSON object".to_string())?;
    let mut question = object
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut header = object.get("header").and_then(Value::as_str).map(str::to_string);
    let mut options = options_of(object)?;
    let mut multi_select = multi_select_of(object)?;

    if let Ok(Value::Object(inner)) = serde_json::from_str::<Value>(question.trim()) {
        if let Some(inner_question) = inner.get("question").and_then(Value::as_str) {
            if options.is_empty() {
                options = options_of(&inner).unwrap_or_default();
            }
            if header.is_none() {
                header = inner.get("header").and_then(Value::as_str).map(str::to_string);
            }
            if multi_select.is_none() {
                multi_select = multi_select_of(&inner).unwrap_or_default();
            }
            question = inner_question.to_string();
        }
    }

    if options.is_empty() {
        if let Some(at) = question.find('[') {
            let (prose, tail) = question.split_at(at);
            if let Ok(found) = serde_json::from_str::<Vec<QuestionOption>>(tail.trim()) {
                if !found.is_empty() {
                    options = found;
                    question = prose.trim().to_string();
                }
            }
        }
    }

    if question.trim().is_empty() {
        return Err("AskUserQuestion requires a non-empty `question`".to_string());
    }
    Ok(AskedQuestion {
        question,
        header,
        options,
        multi_select: multi_select.unwrap_or(false),
    })
}

/// `options`, an array or that array double-encoded as a string.
fn options_of(object: &Map<String, Value>) -> Result<Vec<QuestionOption>, String> {
    let value = match object.get("options") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::String(raw)) => serde_json::from_str(raw.trim())
            .map_err(|error| format!("`options` was a string but not valid JSON: {error}"))?,
        Some(other) => other.clone(),
    };
    serde_json::from_value(value).map_err(|error| format!("`options`: {error}"))
}

/// `multiSelect` (or `multi_select`): a boolean, or the words `true` /
/// `false` a model sends when the schema never reached the provider — the
/// tool is deferred (t-68770). Any other value is an error.
fn multi_select_of(object: &Map<String, Value>) -> Result<Option<bool>, String> {
    let Some(value) = object.get("multiSelect").or_else(|| object.get("multi_select")) else {
        return Ok(None);
    };
    match value {
        Value::Bool(flag) => Ok(Some(*flag)),
        Value::String(word) if word.trim().eq_ignore_ascii_case("true") => Ok(Some(true)),
        Value::String(word) if word.trim().eq_ignore_ascii_case("false") => Ok(Some(false)),
        other => Err(format!("`multiSelect`: expected a boolean, got {other}")),
    }
}

/// A numeric pick (`2`, the stdio form) as its option's label; anything else
/// as typed, trimmed.
#[must_use]
pub fn resolve_choice(answer: &str, options: &[QuestionOption]) -> String {
    let trimmed = answer.trim();
    if let Ok(index) = trimmed.parse::<usize>() {
        if (1..=options.len()).contains(&index) {
            return options[index - 1].label.clone();
        }
    }
    trimmed.to_string()
}

/// What the model reads back once every question is answered: `answers`
/// maps each question to its answer — a string, or an array for a
/// multi-select question. `answers[i]` are question `i`'s picks, numeric
/// ones already resolved.
#[must_use]
pub fn answered_payload(questions: &[AskedQuestion], answers: &[Vec<String>]) -> Value {
    let mut map = Map::new();
    for (question, picks) in questions.iter().zip(answers) {
        let answer = if question.is_multi() {
            json!(picks)
        } else {
            json!(picks.first().cloned().unwrap_or_default())
        };
        map.insert(question.question.clone(), answer);
    }
    let mut payload = json!({ "answers": map, "status": "answered" });
    if let [only] = questions {
        payload["question"] = json!(only.question);
        payload["answer"] = payload["answers"][only.question.as_str()].clone();
    }
    payload
}

/// What a run nobody can answer returns: the questions, unanswered.
#[must_use]
pub fn unanswered_payload(questions: &[AskedQuestion]) -> Value {
    let mut payload = json!({
        "questions": questions.iter().map(|question| question.question.clone()).collect::<Vec<_>>(),
        "status": "unanswered",
        "reason": "non-interactive"
    });
    if let [only] = questions {
        payload["question"] = json!(only.question);
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::{answered_payload, parse_questions, resolve_choice, QuestionStep, MAX_QUESTIONS};
    use serde_json::json;

    fn option(label: &str) -> serde_json::Value {
        json!({ "label": label, "description": format!("what {label} changes") })
    }

    fn entry(question: &str, header: &str, options: usize) -> serde_json::Value {
        json!({
            "question": question,
            "header": header,
            "options": (0..options).map(|index| option(&format!("choice {index}"))).collect::<Vec<_>>()
        })
    }

    #[test]
    fn the_questions_array_reads_each_question_in_order() {
        let parsed = parse_questions(&json!({ "questions": [
            entry("Which store keeps the cache?", "Cache", 2),
            { "question": "Which checks run?", "header": "Checks", "options": [option("unit"), option("e2e"), option("lint")], "multiSelect": true }
        ]}))
        .expect("two questions");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].question, "Which store keeps the cache?");
        assert_eq!(parsed[0].header.as_deref(), Some("Cache"));
        assert_eq!(parsed[0].options[1].description.as_deref(), Some("what choice 1 changes"));
        assert!(!parsed[0].is_multi());
        assert!(parsed[1].is_multi());
    }

    #[test]
    fn a_flat_question_is_a_list_of_one_with_header_and_options_optional() {
        let parsed = parse_questions(&json!({ "question": "Continue?" })).expect("old shape");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].question, "Continue?");
        assert!(parsed[0].header.is_none() && parsed[0].options.is_empty());
        let parsed = parse_questions(&json!({ "question": "Pick", "options": ["a", "b", "c", "d", "e"] })).expect("old shape keeps its options");
        assert_eq!(parsed[0].options.len(), 5);
    }

    #[test]
    fn a_wrong_questions_array_is_refused_with_what_to_fix() {
        let refused = |value: serde_json::Value| parse_questions(&value).expect_err("refused");
        assert!(refused(json!({ "questions": [] })).contains("1 to 4 questions, not 0"));
        let five: Vec<_> = (0..=MAX_QUESTIONS).map(|index| entry(&format!("Q{index}?"), "H", 2)).collect();
        assert!(refused(json!({ "questions": five })).contains("not 5"));
        assert!(refused(json!({ "questions": [entry("Q?", "H", 1)] })).contains("question 1: give 2 to 4 options, not 1"));
        assert!(refused(json!({ "questions": [entry("Q?", "H", 2), entry("R?", "H", 5)] })).contains("question 2: give 2 to 4 options, not 5"));
        assert!(refused(json!({ "questions": [{ "question": "Q?", "options": [option("a"), option("b")] }] })).contains("`header` is required"));
        assert!(refused(json!({ "questions": [entry("", "H", 2)] })).contains("non-empty `question`"));
        assert!(refused(json!({ "questions": "Q?" })).contains("must be an array"));
    }

    #[test]
    fn the_lenient_shapes_still_read() {
        let parsed = parse_questions(&json!({ "question": "제거로 진행?", "options": "[{\"label\": \"제거로 진행\"}, {\"label\": \"유지\"}]" })).expect("double-encoded options");
        assert_eq!(parsed[0].options[1].label, "유지");
        assert!(parse_questions(&json!({ "question": "?", "options": "not json" })).expect_err("garbage").contains("`options` was a string"));
        assert!(parse_questions(&json!({ "question": "?", "options": ["a", "b"], "multiSelect": " True " })).expect("word")[0].multi_select);
        assert!(!parse_questions(&json!({ "question": "?", "options": ["a", "b"], "multi_select": "false" })).expect("alias")[0].multi_select);
        assert!(parse_questions(&json!({ "question": "?", "multiSelect": "yes" })).expect_err("other word").contains("expected a boolean"));
        let nested = parse_questions(&json!({ "header": "Auth", "question": "{\"question\": \"Which auth?\", \"options\": [\"OAuth\", \"API key\"]}" })).expect("nested");
        assert_eq!(nested[0].question, "Which auth?");
        assert_eq!(nested[0].options.len(), 2);
        let trailing = parse_questions(&json!({ "question": "Which one?\n[{\"label\": \"a\"}, {\"label\": \"b\"}]" })).expect("trailing array");
        assert_eq!(trailing[0].question, "Which one?");
        assert_eq!(trailing[0].options.len(), 2);
    }

    #[test]
    fn answers_come_back_per_question_and_one_question_keeps_its_pair() {
        let two = parse_questions(&json!({ "questions": [
            entry("Which store?", "Store", 2),
            { "question": "Which checks?", "header": "Checks", "options": [option("unit"), option("e2e")], "multiSelect": true }
        ]})).expect("two");
        let payload = answered_payload(&two, &[vec!["choice 1".into()], vec!["unit".into(), "e2e".into()]]);
        assert_eq!(payload["answers"]["Which store?"], "choice 1");
        assert_eq!(payload["answers"]["Which checks?"], json!(["unit", "e2e"]));
        assert_eq!(payload["status"], "answered");
        assert!(payload.get("answer").is_none());
        let one = parse_questions(&json!({ "question": "Continue?" })).expect("one");
        let payload = answered_payload(&one, &[vec!["yes".into()]]);
        assert_eq!(payload["question"], "Continue?");
        assert_eq!(payload["answer"], "yes");
        assert_eq!(payload["answers"]["Continue?"], "yes");
    }

    #[test]
    fn a_numeric_pick_is_its_label_and_a_step_knows_its_place() {
        let options = parse_questions(&json!({ "question": "?", "options": ["alpha", "beta"] })).expect("q").remove(0).options;
        assert_eq!(resolve_choice("2", &options), "beta");
        assert_eq!(resolve_choice(" 3 ", &options), "3");
        assert_eq!(resolve_choice(" typed ", &options), "typed");
        let step = QuestionStep { index: 1, headers: vec!["A".into(), "B".into()], answered: Vec::new() };
        assert_eq!(step.total(), 2);
        assert!(step.is_last());
        assert!(!QuestionStep { index: 0, ..step }.is_last());
    }
}
