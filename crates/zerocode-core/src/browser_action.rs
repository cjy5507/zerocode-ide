//! Choosing one of the actions a look already numbered
//! (docs/design/jev-browser-action-20260917.md §2.2): the question a stopped
//! walk asks about the screen in front of it, and what makes an answer valid.
//!
//! Nothing here touches the network, the clock or a page. The wire is the
//! window's (`crates/zerocode-shell/src/systemone.rs`) and what it sends is
//! what this module rendered, so the words that define the question live in
//! exactly one place.
//!
//! The rule the whole design rests on: **the answer space is closed.** A look
//! hands in the controls it saw, this module offers exactly those numbers plus
//! [`GIVE_UP`], and an answer naming anything else is refused whole. A
//! judgment cannot reach an element the look did not see, and it never writes
//! a selector, a URL, a script or a value — it writes a number, which the
//! caller spends on `click --mark <n>`, through the same pin and the same
//! fingerprint every other press goes through.
//!
//! What a look may put in the state is likewise narrow. A step's argv is NOT
//! state: `type <label> <css> <text>` carries the text a person typed, so
//! [`ActionLook::step`] takes the verb alone.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::computer_use_protocol::marks::legend_line;
/// Every way an answer fails to be one — the closed choice's own rules, which
/// every Jev question with a closed answer space keeps (`crate::jev::choice`).
pub use crate::jev::choice::ChoiceRefusal as ActionRefusal;

/// How many of a look's numbers one question may offer. The numbers
/// themselves are already capped by
/// [`crate::computer_use::MARK_CAP`] (99); a choice with ninety-nine options
/// is not a question worth asking, so the judgment keeps a tighter cap of its
/// own. The slice that is cut and the slice that may be chosen from are the
/// same slice — offering a number the reader then cannot pick is how one
/// screen comes to have two truths. The number is the Jev use table's
/// ([`crate::jev::BROWSER`]), where every use's caps are read from.
pub const MAX_ACTION_CANDIDATES: usize = crate::jev::BROWSER_CANDIDATE_CAP;

/// The option that means "none of these would help".
pub const GIVE_UP: &str = "give_up";

/// What a mark's option is called. The number after it is the number the look
/// handed out, so the caller spends the answer without a table of its own.
const MARK_OPTION_PREFIX: &str = "mark:";

/// The one question's name. The endpoint does not show a question's name to
/// the model, so this is the caller's key and nothing more.
const QUESTION: &str = "action";

/// The words of the question. They are ours: a page's own text reaches the
/// model as an option's description and as state, never as an instruction.
const INSTRUCTIONS: &str = "A recorded browser flow stopped at the step named in `stopped`, for the reason in `refusal`. The controls the page is showing right now are the options, each named by the number the screen drew on it. Choose the one control a person would press so that the stopped step can run again.";

/// What choosing [`GIVE_UP`] means. Written as a situation rather than as a
/// degree, because each option is judged on its own words.
const GIVE_UP_MEANS: &str =
    "No control on this screen would let the stopped step run again — the flow needs a person.";

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 6] = ["goal", "stopped", "step", "refusal", "page", "alreadyTried"];

/// The version of the words above. Bump it when any of them changes: a
/// judgment read under one wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to
/// [`rubric_fingerprint`], so changing a word without bumping the version is
/// a red test rather than a quiet drift.
pub const BROWSER_ACTION_RUBRIC_VERSION: u32 = 1;

/// The screen a stopped walk is looking at, as the question reads it.
#[derive(Debug, Clone, Copy)]
pub struct ActionLook<'a> {
    /// What the flow is for — its name, or the check that stopped.
    pub goal: &'a str,
    /// Why the walk stopped, in the walk's own word (`step_failed`, …).
    pub stopped: &'a str,
    /// The stopped step's VERB alone. Never its argv: a `type` line carries
    /// what a person typed.
    pub step: &'a str,
    /// What the step was refused with.
    pub refusal: &'a str,
    /// The page's host and its path — never its query, which carries tokens.
    pub host: &'a str,
    pub path: &'a str,
    /// Numbers this recovery already spent. They are not offered again, so a
    /// second attempt cannot repeat the first.
    pub tried: &'a [usize],
    /// The marks answer's items, in the order the look numbered them.
    pub items: &'a [Value],
}

/// One question, ready for the wire, holding the closed set it offered.
///
/// [`Self::read`] is a method rather than a free function so that an answer is
/// always judged against the very set that was asked — the two cannot drift
/// apart at a call site.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`.
    pub questions: Value,
    /// The numbers offered, in the order they were offered.
    marks: Vec<usize>,
}

/// What an answer chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chosen {
    /// Press this number.
    Mark(usize),
    /// Nothing here helps.
    GiveUp,
}

/// A validated answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionChoice {
    pub chosen: Chosen,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// A mark's option name.
fn option_of(mark: usize) -> String {
    format!("{MARK_OPTION_PREFIX}{mark}")
}

/// The number an option names, if it names one.
fn mark_of(option: &str) -> Option<usize> {
    option.strip_prefix(MARK_OPTION_PREFIX)?.parse().ok()
}

/// The words that define the question, as one string. The version is pinned
/// to this, not to a date or to a reviewer's memory.
#[must_use]
pub fn rubric_words() -> String {
    let mut words = String::new();
    words.push_str(INSTRUCTIONS);
    words.push('\n');
    words.push_str(GIVE_UP);
    words.push('\n');
    words.push_str(GIVE_UP_MEANS);
    words.push('\n');
    words.push_str(MARK_OPTION_PREFIX);
    words.push('\n');
    words.push_str(&STATE_KEYS.join(","));
    words.push('\n');
    words.push_str("candidates");
    words
}

/// The first sixteen hex digits of the words' SHA-256.
#[must_use]
pub fn rubric_fingerprint() -> String {
    crate::jev::words_fingerprint(&rubric_words())
}

/// The question this look asks, or `None` when there is nothing left to
/// choose between — no control the look saw that this recovery has not
/// already spent. A caller that gets `None` stops exactly as it would have
/// without a judgment at all.
#[must_use]
pub fn ask(look: &ActionLook<'_>) -> Option<ActionAsk> {
    let tried: BTreeSet<usize> = look.tried.iter().copied().collect();
    let mut marks = Vec::new();
    let mut criteria = Map::new();
    let mut candidates = Vec::new();
    for item in look.items {
        if marks.len() == MAX_ACTION_CANDIDATES {
            break;
        }
        let Some(mark) = item.get("mark").and_then(Value::as_u64) else {
            continue;
        };
        let Ok(mark) = usize::try_from(mark) else {
            continue;
        };
        if tried.contains(&mark) {
            continue;
        }
        let Some(line) = legend_line(item) else {
            continue;
        };
        criteria.insert(option_of(mark), Value::String(line.clone()));
        candidates.push(Value::String(line));
        marks.push(mark);
    }
    if marks.is_empty() {
        return None;
    }
    criteria.insert(
        GIVE_UP.to_string(),
        Value::String(GIVE_UP_MEANS.to_string()),
    );

    let state = json!({
        STATE_KEYS[0]: look.goal,
        STATE_KEYS[1]: look.stopped,
        STATE_KEYS[2]: look.step,
        STATE_KEYS[3]: look.refusal,
        STATE_KEYS[4]: { "host": look.host, "path": look.path },
        STATE_KEYS[5]: look.tried,
        "candidates": candidates,
    });
    let questions = json!({
        QUESTION: {
            "type": "choice",
            "instructions": INSTRUCTIONS,
            "criteria": Value::Object(criteria),
        }
    });
    Some(ActionAsk {
        state,
        questions,
        marks,
    })
}

impl ActionAsk {
    /// The numbers this question offered, in order.
    #[must_use]
    pub fn marks(&self) -> &[usize] {
        &self.marks
    }

    /// Every option name it offered, in the order a reader would see them.
    #[must_use]
    pub fn options(&self) -> Vec<String> {
        self.marks
            .iter()
            .map(|mark| option_of(*mark))
            .chain(std::iter::once(GIVE_UP.to_string()))
            .collect()
    }

    /// What the endpoint's `answers` map says about this question, judged
    /// against the set this question offered. One broken rule discards the
    /// answer whole: a judgment that got the shape wrong has said nothing
    /// about the screen.
    ///
    /// # Errors
    ///
    /// [`ActionRefusal`] names which rule the answer broke.
    pub fn read(&self, answers: &Value) -> Result<ActionChoice, ActionRefusal> {
        let offered: BTreeSet<String> = self.options().into_iter().collect();
        let choice = crate::jev::choice::read(answers, QUESTION, &offered)?;
        let chosen = if choice.chosen == GIVE_UP {
            Chosen::GiveUp
        } else {
            Chosen::Mark(mark_of(&choice.chosen).ok_or(ActionRefusal::UnknownOption)?)
        };
        Ok(ActionChoice {
            chosen,
            probabilities: choice.probabilities,
            confidence: choice.confidence,
        })
    }
}

#[cfg(test)]
mod tests;
