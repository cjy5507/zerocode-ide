//! What every program that asks a person's decision shares (t-106308): where
//! a decision stands, the wall its point asks inside, what a point lends it
//! and the request built from that, how its answer is read, whether that
//! answer earns its line, and the row its ledger keeps. zo asks at its four
//! points and the window asks for the agents in its panes; both read the
//! answer and write the row here, so one decision's series is one series
//! whichever program asked it.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Answer, Definition, Input, NOT_APPLICABLE, Point, questions, shown_level, store};
use crate::jev::{
    AGENT_TOOL_ASK_OPTIONS, AGENT_TOOL_DEADLINE_MS, COMMAND_GUARD_APPLY_DEADLINE_MS,
    COMMAND_GUARD_TASK_CHAR_CAP, CUSTOM_DECISIONS, Cap, JevMode, SKILL_SEARCH_APPLY_DEADLINE_MS,
    choice, door, noul, project_rules, score, tool_guard,
};

/// A Noul reads yes from half its probability of yes up, so a tie reads yes —
/// and, its two sides level, hands nothing ([`earns_its_line`]).
const YES_FROM: f64 = 0.5;

/// Half a level of a score: a reading this far past a level reads as the next.
const HALF_A_LEVEL: f64 = 0.5;

/// What a point lends the decisions asking there, under each input's key
/// ([`Input::key`]).
pub type Lent = Map<String, Value>;

/// The wall a point's decisions are asked inside: the wall of the seat that
/// already asks there (DESIGN 2.3), read from the use table.
#[must_use]
pub const fn deadline_at(point: Point) -> Duration {
    Duration::from_millis(match point {
        Point::TurnStart => SKILL_SEARCH_APPLY_DEADLINE_MS,
        Point::TurnEnd => project_rules::REQUEST_DEADLINE_MS,
        Point::BeforeCommand => COMMAND_GUARD_APPLY_DEADLINE_MS,
        Point::AgentAsks => AGENT_TOOL_DEADLINE_MS,
    })
}

/// Where a decision stands: off when the switch over every decision or the
/// decision itself is off, on only when both are on, and recording otherwise.
#[must_use]
pub const fn standing(switch: JevMode, own: JevMode) -> JevMode {
    if !switch.asks() || !own.asks() {
        JevMode::Off
    } else if switch.applies() && own.applies() {
        JevMode::On
    } else {
        JevMode::Shadow
    }
}

/// What a shell command about to run lends the decisions asking before one:
/// the command, its folder, and the first line of the person's words with
/// words in it, cut to the command guard's cap
/// ([`COMMAND_GUARD_TASK_CHAR_CAP`]).
#[must_use]
pub fn lent_before_command(command: &str, cwd: &Path, persons_words: &str) -> Lent {
    let task = door::cut(
        &tool_guard::task_line_of(persons_words),
        Cap::Chars(COMMAND_GUARD_TASK_CHAR_CAP),
    );
    [
        (Input::Command.key().to_string(), Value::from(command)),
        (
            Input::Cwd.key().to_string(),
            Value::from(cwd.to_string_lossy().into_owned()),
        ),
        (Input::Task.key().to_string(), Value::from(task)),
    ]
    .into_iter()
    .collect()
}

/// What a turn's end lends the decisions asking there: the turn as the
/// project-rules check reads it.
#[must_use]
pub fn lent_at_turn_end(
    task: String,
    final_text: String,
    evidence: String,
    edited_paths: Vec<String>,
) -> Lent {
    [
        (Input::Task.key().to_string(), Value::from(task)),
        (Input::FinalText.key().to_string(), Value::from(final_text)),
        (Input::Evidence.key().to_string(), Value::from(evidence)),
        (
            Input::EditedPaths.key().to_string(),
            Value::from(edited_paths),
        ),
    ]
    .into_iter()
    .collect()
}

/// What a decision is asked with: the inputs it reads out of what its point
/// lent, and its own question. The door names the model.
#[must_use]
pub fn body_of(definition: &Definition, lent: &Lent) -> Value {
    let state: Map<String, Value> = definition
        .reads
        .iter()
        .filter_map(|input| Some((input.key().to_string(), lent.get(input.key())?.clone())))
        .collect();
    json!({"state": state, "questions": questions(definition)})
}

/// What a decision answered: the word its row keeps, the option its line
/// shows, how sure it was, and every option's probability beside the chosen
/// one's.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub chosen: Value,
    pub shown: String,
    pub confidence: f64,
    pub spread: Vec<f64>,
    pub top: f64,
}

/// The answer of `definition`'s question, `answer` as the reply holds it
/// under its id, read by the reader of its kind; else the word of the
/// reader's rule it broke. A yes or no reads as [`AGENT_TOOL_ASK_OPTIONS`],
/// as sure as its likelier side; a score as its nearest level, shown counted
/// from one over the levels (`3/4`).
///
/// # Errors
/// The token of the first rule the answer breaks.
pub fn read(definition: &Definition, answer: Option<&Value>) -> Result<Reading, String> {
    match &definition.answer {
        Answer::YesNo { .. } => {
            let yes = answer
                .ok_or(noul::NoulRefusal::NoAnswer)
                .and_then(noul::read_value)
                .map_err(|refusal| refusal.token().to_string())?;
            let (word, top) = if yes < YES_FROM {
                (AGENT_TOOL_ASK_OPTIONS[1], 1.0 - yes)
            } else {
                (AGENT_TOOL_ASK_OPTIONS[0], yes)
            };
            Ok(Reading {
                chosen: Value::from(word),
                shown: word.to_string(),
                confidence: top,
                spread: vec![yes, 1.0 - yes],
                top,
            })
        }
        Answer::Choice(options) => {
            let offered: BTreeSet<String> = options
                .keys()
                .cloned()
                .chain([NOT_APPLICABLE.to_string()])
                .collect();
            let chose = answer
                .ok_or(choice::ChoiceRefusal::NoAnswer)
                .and_then(|answer| choice::read_value(answer, &offered))
                .map_err(|refusal| refusal.token().to_string())?;
            let top = chose
                .probabilities
                .get(&chose.chosen)
                .copied()
                .unwrap_or_default();
            Ok(Reading {
                chosen: Value::from(chose.chosen.as_str()),
                confidence: chose.confidence,
                spread: chose.probabilities.values().copied().collect(),
                top,
                shown: chose.chosen,
            })
        }
        Answer::Score(levels) => {
            let answer =
                answer.ok_or_else(|| choice::ChoiceRefusal::NoAnswer.token().to_string())?;
            let scored =
                score::read_value(answer, levels.len()).map_err(|rule| rule.word().to_string())?;
            let level = nearest_level(scored.score, levels.len());
            let probabilities = answer.get("probabilities").and_then(Value::as_object);
            let probability = |value: &Value| value.as_f64().unwrap_or_default();
            Ok(Reading {
                chosen: Value::from(level),
                shown: shown_level(level, levels.len()),
                confidence: scored.confidence,
                spread: probabilities
                    .map(|spread| spread.values().map(probability).collect())
                    .unwrap_or_default(),
                top: probabilities
                    .and_then(|spread| spread.get(&level.to_string()))
                    .map(probability)
                    .unwrap_or_default(),
            })
        }
    }
}

/// Whether an answer earns its decision's line: as sure as the decision's
/// act-from line, its chosen option alone at the top of the spread, an
/// option that applies, and an answer the decision speaks of at its point
/// ([`super::speaks`]: unasked, a yes or no speaks only of its yes). A tie is
/// not an answer the model gave — its side is the order ties break in — so a
/// tie hands nothing at any line and for every kind, the probabilities
/// compared exactly as they were read.
#[must_use]
pub fn earns_its_line(definition: &Definition, reading: &Reading) -> bool {
    let at_the_top = reading
        .spread
        .iter()
        .filter(|probability| **probability >= reading.top)
        .count();
    reading.confidence >= definition.act_from
        && at_the_top == 1
        && reading.shown != NOT_APPLICABLE
        && super::speaks(definition, &reading.shown)
}

/// The level nearest a score's reading, counted from the lowest at zero.
#[must_use]
pub fn nearest_level(reading: f64, levels: usize) -> usize {
    let mut number = 0.0;
    for level in 0..levels {
        if reading < number + HALF_A_LEVEL {
            return level;
        }
        number += 1.0;
    }
    levels.saturating_sub(1)
}

/// One request of a decision's, as its ledger keeps it: which decision and
/// version asked, from which agent, where and under which mode, what became
/// of it, what it chose and how sure, and whether its line was handed. None
/// of the inputs it read; of the person's words, only the option it chose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionRow {
    /// Unix milliseconds when it was asked.
    pub at: u64,
    /// The asking agent's slug: zo's own, or a pane's agent.
    pub from: String,
    /// The decision's id.
    pub decision: String,
    /// The version that asked: its definition's hash.
    pub definition: String,
    /// [`Point::key`].
    pub point: String,
    /// Where the decision stood ([`standing`]).
    pub mode: String,
    pub rubric_version: u32,
    /// [`door::ANSWERED_OUTCOME`], the door's refusal, a failure's ledger
    /// token, or the word of the rule a definition broke when it was checked
    /// again ([`super::Refusal::word`]).
    pub outcome: String,
    /// `yes` or `no`, a choice's option, or a score's level counted from the
    /// lowest at zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Whether its line was handed — at a turn's end, offered to the next.
    /// For a window pane's agent before a command, whether its line was left
    /// waiting for the pane's next tool boundary or brief: a line still
    /// waiting may be dropped there (its decision edited, its folder's
    /// consent withdrawn, no room, another launch), so the count of these is
    /// an upper bound on the lines the agent read.
    pub applied: bool,
    pub elapsed_ms: u64,
    pub requests: u32,
    pub retries: u32,
    pub redacted_lines: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Which rule of the answer's reader refused a reply, on a row whose
    /// outcome says the reply was malformed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
}

impl DecisionRow {
    /// The row of `entry`'s decision, asked from `from` at `at` as `definition`
    /// standing at `mode`, whose request came to `outcome`: nothing sent yet,
    /// nothing chosen, nothing handed.
    #[must_use]
    pub fn of(
        at: u64,
        from: &str,
        entry: &store::Entry,
        definition: &Definition,
        mode: JevMode,
        outcome: &str,
    ) -> Self {
        Self {
            at,
            from: from.to_string(),
            decision: entry.id.clone(),
            definition: entry.definition.clone(),
            point: definition.at.key().to_string(),
            mode: mode.key().to_string(),
            rubric_version: CUSTOM_DECISIONS.rubric_version,
            outcome: outcome.to_string(),
            chosen: None,
            confidence: None,
            applied: false,
            elapsed_ms: 0,
            requests: 0,
            retries: 0,
            redacted_lines: 0,
            input_tokens: None,
            model: None,
            rejected: None,
        }
    }
}
