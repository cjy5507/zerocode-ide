//! The agent tool seat (t-6040): a System One judgment an agent asks for on
//! purpose — zo's `Jev` tool from inside a turn, `zo jev ask|choose|score`
//! from a shell — instead of spending its own model's tokens on a yes/no, a
//! pick or a grading.
//!
//! Every other seat is a stage of the product's own: it asks about words the
//! product chose, and falls back to a reader of its own when nothing answers.
//! This one asks whatever the caller wrote and has nothing to fall back to,
//! which is why its row in the use table (`zerocode_core::jev::AGENT_TOOL`)
//! carries no label, never promotes and offers no `auto`. What it shares is
//! everything else: the door (`jev_gate`), the wire, the ledger discipline,
//! the shared readers of a choice and a score answer, and the dashboard that
//! counts it.
//!
//! One constructor checks a question ([`JevQuestion::new`]), one function
//! decides it ([`decide`]) and one struct is the answer ([`JevVerdict`]),
//! whichever door — the tool or the CLI — it came in by, so the two cannot
//! drift.
//!
//! # What each mode does
//!
//! `off`, the default: nothing is sent and nothing is written; the verdict
//! says so and the caller is where it was before the seat existed. `shadow`:
//! the question is asked and the row written, and the answer is withheld —
//! a person reads the dashboard first and sees what agents ask and what it
//! costs. `on`: the answer is handed over.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{
    SystemOneCall, SystemOneClient, SystemOneConfig, SystemOneFailure, SystemOneQuestion,
    SystemOneRequest, SYSTEMONE_MODEL,
};
use runtime::jev_score::{read_score, Scale};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::shard::even_shards;
use zerocode_core::jev::{
    choice, JevMode, AGENT_TOOL, AGENT_TOOL_ASK_OPTIONS,
    AGENT_TOOL_DEADLINE_MS, AGENT_TOOL_ITEM_CAP, AGENT_TOOL_LEVELS, AGENT_TOOL_OPTION_CAP,
    ROUTE_USE_APPLIED, ROUTE_USE_FALLBACK, SKILL_SHARD_TARGET,
};

use super::jev_gate::{self, JevDoor};
use super::jev_summary::cost_of;
use super::probe_exec::task_fingerprint;
use super::settings::agent_tool_mode_from;
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

/// The seat's ledger file — the use table's name for it.
pub const AGENT_TOOL_FILE: &str = AGENT_TOOL.ledger;

/// Outcome of a row whose judgment answered and checked out: the door's
/// word, which every Jev counter and the hedge rule's sample read.
pub const AGENT_TOOL_OUTCOME_ANSWERED: &str = zerocode_core::jev::door::ANSWERED_OUTCOME;

/// The wall one question holds the caller — the use table's number.
pub const AGENT_TOOL_DEADLINE: Duration = Duration::from_millis(AGENT_TOOL_DEADLINE_MS);

/// Bumped whenever an instruction sentence or the state's shape changes. A
/// row carries it, so a count taken under other words is never read as one
/// about these.
pub const AGENT_TOOL_RUBRIC_VERSION: u32 = 1;

/// What an `ask` is told to do with the state it is handed.
const ASK_INSTRUCTIONS: &str = "Read `context`, then answer `question`: is it yes, or no?";

/// What a `choose` is told to do.
const CHOOSE_INSTRUCTIONS: &str =
    "Read `context`, then answer `question` with the one option that fits it best.";

/// The one question an `ask` asks, by the id its answer comes back under.
const ASK_QUESTION_ID: &str = "a";

/// The one question a `choose` asks.
const CHOOSE_QUESTION_ID: &str = "c";

/// The letter an option's criteria key opens with: a choice's criteria are
/// named by place, never by the caller's words, because a criteria KEY is the
/// one part of a request the door cannot clear — it visits values, not names.
const OPTION_ID_PREFIX: &str = "o";

/// The letter a scored item's question id opens with.
const ITEM_ID_PREFIX: &str = "i";

/// The word a verdict's `routeUse` carries under `off`: the switch's own.
const ROUTE_USE_OFF: &str = "off";

const FAIL_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

/// What a `score` is told to do with `items[k]`.
fn score_instructions(at: usize) -> String {
    format!("Judge `items[{at}]` against `question`: which level describes it?")
}

/// Where a project's agent tool ledger lives.
#[must_use]
pub fn agent_tool_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, AGENT_TOOL_FILE)
}

/// Who asked: the tool from inside a turn, or the CLI from a shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JevCaller {
    Tool,
    Cli,
}

/// The three questions an agent may ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JevShape {
    /// A yes/no — a choice over [`AGENT_TOOL_ASK_OPTIONS`].
    Ask,
    /// One option of the caller's, by place.
    Choose,
    /// Every item of the caller's, on the caller's own levels.
    Score,
}

impl JevShape {
    /// Every shape, in the order the usage lists them.
    pub const ALL: [Self; 3] = [Self::Ask, Self::Choose, Self::Score];

    /// The word a CLI verb, a ledger row and a schema enum spell.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Choose => "choose",
            Self::Score => "score",
        }
    }

    /// The shape `word` names, exactly.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|shape| shape.word() == word)
    }
}

/// Why a question could not be asked as stated — the caller's mistake, said
/// in one sentence, before anything leaves the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JevInvalid(pub String);

impl std::fmt::Display for JevInvalid {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A question as the caller states it, already checked against the table's
/// bounds. Built by [`JevQuestion::new`] and nowhere else, so the tool and
/// the CLI refuse the same things in the same words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevQuestion {
    Ask { question: String, context: Option<String> },
    Choose { question: String, options: Vec<String>, context: Option<String> },
    Score { question: String, levels: Vec<String>, items: Vec<String> },
}

/// The caller's words, trimmed; `None` for words that are only whitespace.
fn trimmed(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Every entry of `list` trimmed, refusing an empty one by its place.
fn trimmed_all(what: &str, list: &[String]) -> Result<Vec<String>, JevInvalid> {
    list.iter()
        .enumerate()
        .map(|(at, text)| trimmed(text).ok_or_else(|| JevInvalid(format!("{what}[{at}] is empty"))))
        .collect()
}

impl JevQuestion {
    /// Check one question against the table's bounds. `options` is read for
    /// `choose`, `levels` and `items` for `score`, `context` for `ask` and
    /// `choose`; a part a shape does not read is refused rather than
    /// dropped, because a caller who handed it over meant something by it.
    ///
    /// # Errors
    /// The first bound the question breaks, in a sentence for the caller.
    pub fn new(
        shape: JevShape,
        question: &str,
        context: Option<&str>,
        options: &[String],
        levels: &[String],
        items: &[String],
    ) -> Result<Self, JevInvalid> {
        let question = trimmed(question).ok_or_else(|| JevInvalid("question is empty".to_string()))?;
        let context = context.and_then(trimmed);
        let unread = |what: &str, given: bool| {
            given.then(|| JevInvalid(format!("`{}` does not read {what}", shape.word())))
        };
        match shape {
            JevShape::Ask => {
                if let Some(refusal) = unread("options", !options.is_empty())
                    .or_else(|| unread("levels", !levels.is_empty()))
                    .or_else(|| unread("items", !items.is_empty()))
                {
                    return Err(refusal);
                }
                Ok(Self::Ask { question, context })
            }
            JevShape::Choose => {
                if let Some(refusal) =
                    unread("levels", !levels.is_empty()).or_else(|| unread("items", !items.is_empty()))
                {
                    return Err(refusal);
                }
                if options.len() < 2 {
                    return Err(JevInvalid("choose needs at least two options".to_string()));
                }
                if options.len() > AGENT_TOOL_OPTION_CAP {
                    return Err(JevInvalid(format!(
                        "choose takes at most {AGENT_TOOL_OPTION_CAP} options, not {}",
                        options.len()
                    )));
                }
                Ok(Self::Choose { question, options: trimmed_all("options", options)?, context })
            }
            JevShape::Score => {
                if let Some(refusal) =
                    unread("options", !options.is_empty()).or_else(|| unread("context", context.is_some()))
                {
                    return Err(refusal);
                }
                if !AGENT_TOOL_LEVELS.contains(&levels.len()) {
                    return Err(JevInvalid(format!(
                        "score takes between {} and {} levels, not {}",
                        AGENT_TOOL_LEVELS.start(),
                        AGENT_TOOL_LEVELS.end(),
                        levels.len()
                    )));
                }
                if items.is_empty() {
                    return Err(JevInvalid("score needs at least one item".to_string()));
                }
                if items.len() > AGENT_TOOL_ITEM_CAP {
                    return Err(JevInvalid(format!(
                        "score takes at most {AGENT_TOOL_ITEM_CAP} items per call, not {}",
                        items.len()
                    )));
                }
                Ok(Self::Score {
                    question,
                    levels: trimmed_all("levels", levels)?,
                    items: trimmed_all("items", items)?,
                })
            }
        }
    }

    /// Which of the three this is.
    #[must_use]
    pub const fn shape(&self) -> JevShape {
        match self {
            Self::Ask { .. } => JevShape::Ask,
            Self::Choose { .. } => JevShape::Choose,
            Self::Score { .. } => JevShape::Score,
        }
    }

    /// How many things the judgment is asked to decide between or about:
    /// the two words of an `ask`, a `choose`'s options, a `score`'s items.
    #[must_use]
    pub fn candidates(&self) -> usize {
        match self {
            Self::Ask { .. } => AGENT_TOOL_ASK_OPTIONS.len(),
            Self::Choose { options, .. } => options.len(),
            Self::Score { items, .. } => items.len(),
        }
    }

    /// A fingerprint of the caller's words — what the row carries in place
    /// of them.
    fn fingerprint(&self) -> u64 {
        let mut words = String::new();
        let mut push = |text: &str| {
            words.push_str(text);
            words.push('\u{1e}');
        };
        match self {
            Self::Ask { question, context } => {
                push(question);
                push(context.as_deref().unwrap_or_default());
            }
            Self::Choose { question, options, context } => {
                push(question);
                push(context.as_deref().unwrap_or_default());
                for option in options {
                    push(option);
                }
            }
            Self::Score { question, levels, items } => {
                push(question);
                for level in levels {
                    push(level);
                }
                for item in items {
                    push(item);
                }
            }
        }
        task_fingerprint(self.shape().word(), &words)
    }
}

/// One shard's request: its state, its questions, and the item indices it
/// asks about (empty for the shapes that ask one question).
struct Asking {
    state: Value,
    questions: BTreeMap<String, SystemOneQuestion>,
    items: std::ops::Range<usize>,
}

/// The requests one question becomes: one for an `ask` or a `choose`, one
/// per even shard of the items for a `score`.
fn requests_of(question: &JevQuestion) -> Vec<Asking> {
    match question {
        JevQuestion::Ask { question, context } => vec![Asking {
            state: json!({ "question": question, "context": context }),
            questions: BTreeMap::from([(
                ASK_QUESTION_ID.to_string(),
                SystemOneQuestion::choice(
                    ASK_INSTRUCTIONS,
                    AGENT_TOOL_ASK_OPTIONS.iter().map(|word| (*word, None)),
                ),
            )]),
            items: 0..0,
        }],
        JevQuestion::Choose { question, options, context } => {
            let ids: Vec<String> =
                (0..options.len()).map(|at| format!("{OPTION_ID_PREFIX}{at}")).collect();
            vec![Asking {
                state: json!({ "question": question, "context": context }),
                questions: BTreeMap::from([(
                    CHOOSE_QUESTION_ID.to_string(),
                    SystemOneQuestion::choice(
                        CHOOSE_INSTRUCTIONS,
                        ids.iter().zip(options).map(|(id, option)| (id.as_str(), Some(option.as_str()))),
                    ),
                )]),
                items: 0..0,
            }]
        }
        JevQuestion::Score { question, levels, items } => even_shards(items.len(), SKILL_SHARD_TARGET)
            .into_iter()
            .map(|shard| Asking {
                state: json!({ "question": question, "items": &items[shard.clone()] }),
                questions: shard
                    .clone()
                    .map(|at| {
                        (
                            format!("{ITEM_ID_PREFIX}{at}"),
                            SystemOneQuestion::score(
                                &score_instructions(at - shard.start),
                                levels.iter().map(String::as_str),
                            ),
                        )
                    })
                    .collect(),
                items: shard,
            })
            .collect(),
    }
}

/// One item's reading, as a `score` hands it back.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoredItem {
    /// The item's place in what the caller handed over.
    pub index: usize,
    /// The caller's own words, back to the caller.
    pub item: String,
    /// The position along the levels, `0` to the top level's number.
    pub score: f64,
    /// The same reading on 0 to 1.
    pub normalised: f64,
    pub confidence: f64,
}

/// What a judgment answered, in the shape of the question.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum JevAnswer {
    #[serde(rename_all = "camelCase")]
    Ask {
        yes: bool,
        /// The probability the judgment put on the affirmative.
        p_yes: f64,
        confidence: f64,
    },
    #[serde(rename_all = "camelCase")]
    Choose {
        /// The chosen option's own words.
        chosen: String,
        /// Its place in the options the caller handed over.
        index: usize,
        /// One probability per option, in the caller's order.
        probabilities: Vec<f64>,
        confidence: f64,
    },
    Score {
        /// Every item a shard answered for, in the caller's order. Fewer
        /// than were asked when a shard was refused; the verdict's note
        /// says how many.
        items: Vec<ScoredItem>,
    },
}

/// The answer as the caller receives it — the one shape the tool prints and
/// the CLI renders.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JevVerdict {
    pub shape: JevShape,
    /// [`AGENT_TOOL_OUTCOME_ANSWERED`], a failure's ledger token, the door's
    /// refusal token, or the switch's own `off`.
    pub outcome: String,
    /// [`ROUTE_USE_APPLIED`] when `answer` is the judgment's and handed over;
    /// `shadow` when it was asked and withheld; [`ROUTE_USE_FALLBACK`] when
    /// nothing answered; `off` when nothing was asked.
    pub route_use: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<JevAnswer>,
    pub elapsed_ms: u64,
    /// Requests that left the machine for this question.
    pub requests: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// What the billed tokens cost at the wire's rate, when the price table
    /// names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// One sentence for the caller: what the outcome means for it.
    pub note: String,
}

impl JevVerdict {
    /// Whether the caller was handed the judgment's answer.
    #[must_use]
    pub fn answered(&self) -> bool {
        self.answer.is_some()
    }
}

/// One question's row: what was asked of the wire and what came back, with
/// none of the caller's words — a fingerprint stands for them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentToolRow {
    /// Unix milliseconds when the row was made.
    pub at: u64,
    pub caller: JevCaller,
    pub shape: JevShape,
    /// Fingerprint of the caller's words.
    pub question: u64,
    #[serde(rename = "rubricVersion")]
    pub rubric_version: u32,
    /// [`AGENT_TOOL_OUTCOME_ANSWERED`], a failure's ledger token, or the
    /// door's refusal token.
    pub outcome: String,
    /// What the judgment decided between or about ([`JevQuestion::candidates`]).
    pub candidates: usize,
    /// Requests the question was cut into, and how many came back checked.
    pub shards: usize,
    #[serde(rename = "shardsAnswered")]
    pub shards_answered: usize,
    /// [`ROUTE_USE_APPLIED`], `shadow`, or [`ROUTE_USE_FALLBACK`].
    #[serde(rename = "routeUse")]
    pub route_use: String,
    /// Always false: an agent's question is its own, and is never answered
    /// from a memo. Written so the shared readers find the column.
    #[serde(default)]
    pub cached: bool,
    #[serde(default, rename = "elapsedMs")]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub retries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "inputTokens")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "redactedLines")]
    pub redacted_lines: Option<u32>,
    /// Which rule refused a reply, on a row whose outcome says a reply was
    /// refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
    /// What the judgment chose, on an `ask` (`yes`/`no`) or a `choose` (the
    /// option's place, `o3`) — never the caller's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "hedgeDelayMs")]
    pub hedge_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "hedgeFired")]
    pub hedge_fired: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "hedgeWon")]
    pub hedge_won: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "loserMs")]
    pub loser_ms: Option<u64>,
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

impl AgentToolRow {
    fn new(caller: JevCaller, question: &JevQuestion, shards: usize, outcome: String) -> Self {
        Self {
            at: unix_millis(),
            caller,
            shape: question.shape(),
            question: question.fingerprint(),
            rubric_version: AGENT_TOOL_RUBRIC_VERSION,
            outcome,
            candidates: question.candidates(),
            shards,
            shards_answered: 0,
            route_use: ROUTE_USE_FALLBACK.to_string(),
            cached: false,
            elapsed_ms: 0,
            retries: 0,
            model: None,
            input_tokens: None,
            requests: Some(0),
            redacted_lines: Some(0),
            rejected: None,
            chosen: None,
            confidence: None,
            hedge_delay_ms: None,
            hedge_fired: None,
            hedge_won: None,
            loser_ms: None,
        }
    }
}

/// The mode this question is to be judged under, or `None` when it is not
/// to be judged at all: an ablation, an unreadable setting, or `off`.
fn asking_mode(cwd: &Path) -> Option<JevMode> {
    if telemetry::attest_ablated(telemetry::HarnessFeature::AgentTool) {
        return None;
    }
    let Some(mode) = agent_tool_mode_from(&runtime::ConfigLoader::default_for(cwd)) else {
        telemetry::attest_failed(telemetry::HarnessFeature::AgentTool, FAIL_SETTINGS_UNAVAILABLE);
        return None;
    };
    if !mode.asks() {
        telemetry::attest_declined(telemetry::HarnessFeature::AgentTool, mode.key());
        return None;
    }
    Some(mode)
}

/// Decide `question` for `caller`, on the road the project under `cwd` has
/// its switch set to.
///
/// Always answers with a verdict: `off` is a verdict that nothing was asked,
/// and every failure is one that says why. The row, when one is owed, is
/// appended before this returns.
#[must_use]
pub fn decide(cwd: &Path, caller: JevCaller, question: &JevQuestion) -> JevVerdict {
    let Some(mode) = asking_mode(cwd) else {
        return JevVerdict {
            shape: question.shape(),
            outcome: ROUTE_USE_OFF.to_string(),
            route_use: ROUTE_USE_OFF.to_string(),
            answer: None,
            elapsed_ms: 0,
            requests: 0,
            input_tokens: None,
            cost_usd: None,
            model: None,
            note: format!(
                "smart.{} is off, so nothing was sent; decide this yourself, or ask the person to \
                 set it to shadow or on in the ZeroCode settings.",
                AGENT_TOOL.setting
            ),
        };
    };
    let acting = mode.applies();
    let (mut row, answer, asked) = api::sync_bridge::run_blocking(judge(cwd, caller, question, acting));
    let answered = row.outcome == AGENT_TOOL_OUTCOME_ANSWERED;
    row.route_use = if !answered {
        ROUTE_USE_FALLBACK.to_string()
    } else if acting {
        ROUTE_USE_APPLIED.to_string()
    } else {
        mode.key().to_string()
    };
    let _ = append_shadow_row(&agent_tool_path(cwd), &row, SHADOW_LEDGER_MAX_BYTES);
    let note = match (&answer, acting) {
        (Some(JevAnswer::Score { items }), true) if items.len() < question.candidates() => format!(
            "{} of {} items were scored; the rest were in a request that was refused ({}).",
            items.len(),
            question.candidates(),
            row.rejected.as_deref().unwrap_or(&row.outcome)
        ),
        (Some(_), true) => "Jev answered; the probabilities are the whole of its reasoning.".to_string(),
        (Some(_), false) => format!(
            "smart.{} is {}, so the answer was recorded for the person and withheld from you; \
             decide this yourself.",
            AGENT_TOOL.setting,
            mode.key()
        ),
        (None, _) => format!(
            "No answer ({}); decide this yourself.",
            row.rejected.as_deref().unwrap_or(&row.outcome)
        ),
    };
    JevVerdict {
        shape: question.shape(),
        outcome: row.outcome.clone(),
        route_use: row.route_use.clone(),
        answer: answer.filter(|_| acting),
        elapsed_ms: row.elapsed_ms,
        requests: row.requests.unwrap_or(0),
        input_tokens: row.input_tokens,
        cost_usd: row.input_tokens.and_then(|tokens| cost_of(tokens, &asked)),
        model: row.model.clone(),
        note,
    }
}

/// One question's row and what it answered: refused at the door, or asked in
/// its requests at once and checked — with the model the door asked for, the
/// id the verdict's cost is priced by.
async fn judge(
    cwd: &Path,
    caller: JevCaller,
    question: &JevQuestion,
    acting: bool,
) -> (AgentToolRow, Option<JevAnswer>, String) {
    let askings = requests_of(question);
    let cwd_owned = cwd.to_path_buf();
    let Ok(door) = tokio::task::spawn_blocking(move || JevDoor::open(&cwd_owned)).await else {
        telemetry::attest_failed(telemetry::HarnessFeature::AgentTool, FAIL_SETTINGS_UNAVAILABLE);
        return (
            AgentToolRow::new(caller, question, askings.len(), FAIL_SETTINGS_UNAVAILABLE.to_string()),
            None,
            SYSTEMONE_MODEL.to_string(),
        );
    };
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    // Every request at once: a score's shards are independent requests over
    // disjoint items, and the caller waits for the slowest either way.
    let asked = askings.iter().map(|asking| ask_one(&door, client.as_ref(), asking, acting));
    let replies = futures_util::future::join_all(asked).await;
    let (row, answer) = fold(caller, question, &askings, replies);
    (row, answer, door.model().to_string())
}

/// What one request came back with.
struct Reply {
    /// The reading, or why there is none.
    read: Result<Read, Refusal>,
    call: Option<SystemOneCall>,
    withheld: u32,
}

/// One request's checked answer.
enum Read {
    Choice(choice::Choice),
    Scores(Vec<(usize, runtime::jev_score::ScoreReading)>),
}

/// Why a request produced no reading, in the words a ledger row keeps.
struct Refusal {
    outcome: String,
    rejected: Option<String>,
}

/// Ask one request, through the door.
async fn ask_one(
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    asking: &Asking,
    acting: bool,
) -> Reply {
    let refused = |outcome: String| Reply {
        read: Err(Refusal { outcome, rejected: None }),
        call: None,
        withheld: 0,
    };
    let request =
        SystemOneRequest { state: &asking.state, model: SYSTEMONE_MODEL, questions: &asking.questions };
    let Some(body) = jev_gate::body_of(&request) else {
        let failure = SystemOneFailure::InvalidRequest;
        telemetry::attest_failed(telemetry::HarnessFeature::AgentTool, failure.token());
        return refused(failure.ledger_token());
    };
    let (cleared, client) = match (door.pass(&AGENT_TOOL, client.is_some(), body), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        // The door refuses a keyless request before anything else it asks.
        (passed, _) => {
            let refusal = passed.err().unwrap_or(Refused::NoKey);
            telemetry::attest_declined(telemetry::HarnessFeature::AgentTool, refusal.token());
            return refused(refusal.token().to_string());
        }
    };
    let withheld = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    // A hedge buys an answer inside a wall, and only an acting question is
    // waited on inside one: a recorded answer nobody receives is not worth a
    // second copy of the person's money.
    let hedge = door.hedge_now(&AGENT_TOOL, AGENT_TOOL_DEADLINE, acting);
    let call = jev_gate::send(client, cleared, AGENT_TOOL_DEADLINE, hedge).await;
    let read = match &call.outcome {
        Ok(response) => read_reply(asking, response),
        Err(failure) => Err(Refusal { outcome: failure.ledger_token(), rejected: None }),
    };
    Reply { read, call: Some(call), withheld }
}

/// Check one reply against the questions its request asked.
fn read_reply(asking: &Asking, response: &api::SystemOneResponse) -> Result<Read, Refusal> {
    let schema = |rule: String| Refusal {
        outcome: SystemOneFailure::Schema.ledger_token(),
        rejected: Some(rule),
    };
    if asking.items.is_empty() {
        // One choice question: the shared reader of a closed choice.
        let (id, offered): (&str, BTreeSet<String>) = match asking.questions.iter().next() {
            Some((id, question)) => (
                id.as_str(),
                question.criteria.options().map(str::to_string).collect(),
            ),
            None => return Err(schema("no_question".to_string())),
        };
        let answers = serde_json::to_value(&response.answers).map_err(|_| schema("not_json".to_string()))?;
        return choice::read(&answers, id, &offered)
            .map(Read::Choice)
            .map_err(|refusal| schema(refusal.token().to_string()));
    }
    // One score question per item: the shared reader of a score, on the
    // caller's own levels.
    let asked: BTreeSet<&str> = asking.questions.keys().map(String::as_str).collect();
    if let Some(stray) = response.answers.keys().find(|id| !asked.contains(id.as_str())) {
        return Err(schema(format!("unknown_answer:{stray}")));
    }
    let levels: Vec<&str> = asking
        .questions
        .values()
        .next()
        .and_then(|question| question.criteria.levels())
        .map(|levels| levels.iter().map(String::as_str).collect())
        .ok_or_else(|| schema("no_levels".to_string()))?;
    let scale = Scale::new(&levels);
    asking
        .items
        .clone()
        .map(|at| {
            let id = format!("{ITEM_ID_PREFIX}{at}");
            let answer = response
                .score_answer(&id)
                .ok_or_else(|| schema("missing_answer".to_string()))?
                .map_err(|_| schema("not_a_score".to_string()))?;
            read_score(&answer, &scale)
                .map(|reading| (at, reading))
                .map_err(|rule| schema(rule.word().to_string()))
        })
        .collect::<Result<Vec<_>, Refusal>>()
        .map(Read::Scores)
}

/// The answer in the shape of the question, from what the replies read —
/// and the one word and confidence the row keeps of it.
fn answer_of(
    question: &JevQuestion,
    choice: Option<choice::Choice>,
    mut scored: Vec<(usize, runtime::jev_score::ScoreReading)>,
    row: &mut AgentToolRow,
) -> Option<JevAnswer> {
    match (question, choice) {
        (JevQuestion::Ask { .. }, Some(read)) => {
            row.chosen = Some(read.chosen.clone());
            row.confidence = Some(read.confidence);
            Some(JevAnswer::Ask {
                yes: read.chosen == AGENT_TOOL_ASK_OPTIONS[0],
                p_yes: read.probabilities.get(AGENT_TOOL_ASK_OPTIONS[0]).copied().unwrap_or(0.0),
                confidence: read.confidence,
            })
        }
        (JevQuestion::Choose { options, .. }, Some(read)) => {
            row.chosen = Some(read.chosen.clone());
            row.confidence = Some(read.confidence);
            let index = read
                .chosen
                .strip_prefix(OPTION_ID_PREFIX)
                .and_then(|at| at.parse::<usize>().ok())
                .unwrap_or_default();
            Some(JevAnswer::Choose {
                chosen: options.get(index).cloned().unwrap_or_default(),
                index,
                probabilities: (0..options.len())
                    .map(|at| {
                        read.probabilities
                            .get(&format!("{OPTION_ID_PREFIX}{at}"))
                            .copied()
                            .unwrap_or(0.0)
                    })
                    .collect(),
                confidence: read.confidence,
            })
        }
        (JevQuestion::Score { items, .. }, _) => {
            scored.sort_by_key(|(at, _)| *at);
            Some(JevAnswer::Score {
                items: scored
                    .into_iter()
                    .map(|(at, reading)| ScoredItem {
                        index: at,
                        item: items.get(at).cloned().unwrap_or_default(),
                        score: reading.score,
                        normalised: reading.normalised,
                        confidence: reading.confidence,
                    })
                    .collect(),
            })
        }
        (JevQuestion::Ask { .. } | JevQuestion::Choose { .. }, None) => None,
    }
}

/// Fold every request's reply into one row and one answer.
///
/// A request that was refused takes its items out of the answer and nothing
/// else; the others asked about different items and their readings stand.
/// The row's outcome is the question's — answered when anything was, and
/// the first refusal's word when nothing was.
fn fold(
    caller: JevCaller,
    question: &JevQuestion,
    askings: &[Asking],
    replies: Vec<Reply>,
) -> (AgentToolRow, Option<JevAnswer>) {
    let mut row = AgentToolRow::new(caller, question, askings.len(), String::new());
    let mut choice = None;
    let mut scored: Vec<(usize, runtime::jev_score::ScoreReading)> = Vec::new();
    let mut first_refusal: Option<Refusal> = None;
    let (mut requests, mut withheld, mut input_tokens, mut elapsed, mut retries) = (0, 0, 0, 0, 0);
    for reply in replies {
        withheld += reply.withheld;
        if let Some(call) = &reply.call {
            requests += call.requests;
            retries = retries.max(call.retries);
            // The slowest request is what the caller waited: they left together.
            elapsed = elapsed.max(jev_gate::millis(call.elapsed));
            if let Ok(response) = &call.outcome {
                input_tokens += response.usage.input_tokens;
                row.model.get_or_insert_with(|| response.model.clone());
            }
            if let Some(ran) = call.hedge.filter(|_| row.hedge_fired.is_none()) {
                row.hedge_delay_ms = Some(jev_gate::millis(ran.delay));
                row.hedge_fired = Some(true);
                row.hedge_won = Some(ran.won);
                row.loser_ms = ran.loser_ms;
            }
        }
        match reply.read {
            Ok(Read::Choice(read)) => {
                row.shards_answered += 1;
                choice = Some(read);
            }
            Ok(Read::Scores(read)) => {
                row.shards_answered += 1;
                scored.extend(read);
            }
            Err(refusal) => {
                if first_refusal.is_none() {
                    first_refusal = Some(refusal);
                }
            }
        }
    }
    row.requests = Some(requests);
    row.redacted_lines = Some(withheld);
    row.input_tokens = (input_tokens > 0).then_some(input_tokens);
    row.elapsed_ms = elapsed;
    row.retries = retries;
    // A refused shard beside answered ones is still worth its word on the row.
    row.rejected = first_refusal.as_ref().and_then(|refusal| refusal.rejected.clone());
    if row.shards_answered > 0 {
        telemetry::attest_fired(telemetry::HarnessFeature::AgentTool);
        row.outcome = AGENT_TOOL_OUTCOME_ANSWERED.to_string();
        let Some(answer) = answer_of(question, choice, scored, &mut row) else {
            // A choice question that "answered" without a reading cannot
            // happen — the reading IS the answer — but a row must still say
            // something true if it ever did.
            row.outcome = SystemOneFailure::Schema.ledger_token();
            return (row, None);
        };
        return (row, Some(answer));
    }
    let refusal = first_refusal.unwrap_or(Refusal {
        outcome: SystemOneFailure::NoKey.ledger_token(),
        rejected: None,
    });
    row.outcome = refusal.outcome;
    (row, None)
}

#[cfg(test)]
mod tests;
