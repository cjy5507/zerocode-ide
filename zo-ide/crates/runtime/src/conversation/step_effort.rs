//! The step effort governor — the reasoning effort a request carries is
//! decided per STEP of the turn, not once per turn
//! (docs/design/zo-step-effort-governor-20260921.md, t-5633).
//!
//! A turn's effort used to be one value: the person's `--effort`, static or
//! a Smart band, put on the client at turn start and sent unchanged by every
//! request of the agent loop. The signals that say a step is stuck were all
//! already here — the tool-fingerprint repetition tally, the check-shaped
//! bash command that did not exit 0, a run of tool errors — and were spent on
//! nudges and hard stops only. This module reads them through ONE table
//! ([`decide`]) and moves the next request's effort by a rung or two: up when
//! the step is stuck, down when the last batch was routine reads, unchanged
//! while the model is doing work. On the strong signals alone it may also
//! move the model a rung, through the same-provider wire override the
//! runtime already has (`escalation_model_override`), never through a road of
//! its own.
//!
//! What it deliberately is not:
//!
//! - a second effort→wire mapping: the decision lands on the request as
//!   [`EffortStep`] — the same two fields (`effort`, `effort_band_ceiling`)
//!   every backend already reads — and the host's client derives the thinking
//!   budget from the same level ladder it always did;
//! - a question to Jev every step: the seat a host installs
//!   ([`StepEffortSeat`]) is asked only when the signals disagree with one
//!   another or every [`STEP_JUDGMENT_EVERY`] steps, detached, and its answer
//!   is read at the NEXT step;
//! - a writer: every decision is told to the observer the host installs
//!   ([`StepEffortObserver`]), which files it; the runtime keeps no ledger.
//!
//! Applying is the host's word (`smart.zoStepEffort`): with `applies` false
//! the table runs and its rows are filed and the requests go out untouched,
//! which is the shadow the A/B is read from.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use ::api::{EffortLevel, SystemOneQuestion, SystemOneRequest};
use serde::Serialize;
use serde_json::{json, Value};
use zerocode_core::jev::questions::{
    ROUTING_COMPLEXITY_LEVELS, ROUTING_INTENT_QUESTION, ROUTING_RISK_LEVELS, ROUTING_RISK_QUESTION,
    ROUTING_STATE_TASK,
};

use super::deep_gate::{bash_result_exited_zero, command_is_check_shaped};
use super::repetition::{fingerprint_tool_call, TOOL_REPETITION_THRESHOLD};
use super::tool::is_concurrency_safe;
use super::{ApiClient, ContentBlock, ConversationMessage, ConversationRuntime, ToolExecutor};
use crate::model_router::{RouteTaskComplexity, RubricAxis, COMPLEXITY_AXIS, INTENT_AXIS, RISK_AXIS};
use crate::SwitchTrigger;

/// Every this many steps the seat is asked once, signals or no signals — the
/// `k` of the design: one judgment per five requests keeps the seat's cost
/// under a fifth of a routing probe per turn while still refreshing the band
/// inside a long loop.
pub const STEP_JUDGMENT_EVERY: u32 = 5;

/// Strong signals in a row before the governor moves the model a rung up.
/// One strong step is a rung of effort; a second one on the raised effort is
/// what says the effort was not the wall.
pub const STRONG_STEPS_FOR_HEAVIER: u32 = 2;

/// Routine batches in a row, with the effort already on its floor, before
/// the governor moves the model a rung down. Five, because a model move
/// costs the prompt cache and a read-only stretch shorter than that is not
/// worth paying it for.
pub const ROUTINE_STEPS_FOR_LIGHTER: u32 = 5;

/// Read-only batches in a row a `Large` turn needs before a step reads as
/// routine: one read in a hard task is orientation, not routine.
const LARGE_ROUTINE_STEPS: u32 = 2;

/// Batches with a tool error in a row that read as stuck rather than slipping.
const STUCK_ERROR_STREAK: u32 = 2;

/// What the last tool batch was, read off its results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepBatch {
    /// No batch yet: the turn's first request.
    #[default]
    None,
    /// Every call was a concurrency-safe read, search or lookup.
    ReadOnly,
    /// A file edit or write landed.
    Edit,
    /// A check-shaped shell command ran (`cargo test`, `pytest`, …).
    Check,
    /// Some other shell command ran.
    Exec,
    /// Anything else — agents, todo writes, MCP tools.
    Other,
}

impl StepBatch {
    /// The word a ledger row carries.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ReadOnly => "read_only",
            Self::Edit => "edit",
            Self::Check => "check",
            Self::Exec => "exec",
            Self::Other => "other",
        }
    }
}

/// What the table reads for one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StepSignals {
    pub batch: StepBatch,
    /// The largest per-turn fingerprint count among the batch's calls — `1`
    /// for a call made for the first time this turn, as the repetition tally
    /// counts it.
    pub repeats: usize,
    /// Batches in a row that carried at least one tool error.
    pub error_streak: u32,
    /// A check-shaped shell command in this batch did not exit 0.
    pub check_red: bool,
    /// Read-only batches in a row, this one included.
    pub routine_streak: u32,
    /// The turn's band, as the routing probe or judgment set it.
    pub band: RouteTaskComplexity,
}

/// The seat's answer about a step: the band it read, and the step it was
/// asked at — it applies from the step after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepJudgment {
    pub complexity: RouteTaskComplexity,
    pub at_step: u32,
}

impl StepJudgment {
    /// The rung the band asks for, relative to the turn's effort: an easy
    /// band a rung down, `Large` a rung up, `Medium` none, `Unknown` no
    /// opinion. Never two rungs: that is the table's word for a step that is
    /// stuck, which a judgment about the task cannot see.
    #[must_use]
    pub const fn delta(self) -> Option<i8> {
        match self.complexity {
            RouteTaskComplexity::Trivial | RouteTaskComplexity::Small => Some(-1),
            RouteTaskComplexity::Medium => Some(0),
            RouteTaskComplexity::Large => Some(1),
            RouteTaskComplexity::Unknown => None,
        }
    }

    /// Whether an answer given at `at_step` still speaks for `step`: from the
    /// step after it was asked, for one judgment cadence.
    #[must_use]
    pub const fn fresh_at(self, step: u32) -> bool {
        step > self.at_step && step - self.at_step <= STEP_JUDGMENT_EVERY
    }
}

/// Why the table moved (or did not move) the effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepReason {
    StuckCheckRed,
    StuckRepeat,
    StuckErrors,
    SlippingRepeat,
    SlippingError,
    RoutineReads,
    Working,
    /// The seat's fresh band overrode a weak table reading.
    Judged,
}

impl StepReason {
    /// The word a ledger row carries.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::StuckCheckRed => "stuck_check_red",
            Self::StuckRepeat => "stuck_repeat",
            Self::StuckErrors => "stuck_errors",
            Self::SlippingRepeat => "slipping_repeat",
            Self::SlippingError => "slipping_error",
            Self::RoutineReads => "routine_reads",
            Self::Working => "working",
            Self::Judged => "judged",
        }
    }
}

/// Why the seat is asked at a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepAsk {
    /// The signals disagree: a read-only batch that is also slipping.
    Conflict,
    /// Every [`STEP_JUDGMENT_EVERY`] steps.
    Cadence,
}

impl StepAsk {
    /// The word a ledger row carries.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::Cadence => "cadence",
        }
    }
}

/// What the table decided for the next request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepDecision {
    /// Rungs from the turn's effort: `-1`, `0`, `+1` or `+2`.
    pub delta: i8,
    pub reason: StepReason,
    /// A strong stuck signal: a red check, the repetition advisory's count,
    /// or two error batches in a row.
    pub strong: bool,
    /// A weak stuck signal: a second identical call, one error batch.
    pub slipping: bool,
    /// A routine read-only batch with no stuck signal.
    pub routine: bool,
    /// Whether, and why, the seat is asked at this step.
    pub ask: Option<StepAsk>,
    /// The judgment the decision consulted, when a fresh one stood.
    pub judged: Option<StepJudgment>,
}

/// The table: one step's signals to the next request's effort rung.
///
/// Strong signals win over everything; the seat's fresh band, when one
/// stands, wins over the weaker readings (it was asked because they were
/// weak); the turn's own band decides how far a stuck step may climb and how
/// soon a read-only stretch counts as routine.
#[must_use]
pub fn decide(step: u32, signals: &StepSignals, judgment: Option<StepJudgment>) -> StepDecision {
    let strong = signals.check_red
        || signals.repeats >= TOOL_REPETITION_THRESHOLD
        || signals.error_streak >= STUCK_ERROR_STREAK;
    let slipping = !strong
        && (signals.repeats + 1 == TOOL_REPETITION_THRESHOLD || signals.error_streak == 1);
    let routine_after = match signals.band {
        RouteTaskComplexity::Large => LARGE_ROUTINE_STEPS,
        _ => 1,
    };
    let routine = !strong
        && !slipping
        && signals.batch == StepBatch::ReadOnly
        && signals.routine_streak >= routine_after;
    let easy_band = matches!(
        signals.band,
        RouteTaskComplexity::Trivial | RouteTaskComplexity::Small
    );
    let (table_delta, reason) = if strong {
        let rungs = if easy_band { 1 } else { 2 };
        let reason = if signals.check_red {
            StepReason::StuckCheckRed
        } else if signals.repeats >= TOOL_REPETITION_THRESHOLD {
            StepReason::StuckRepeat
        } else {
            StepReason::StuckErrors
        };
        (rungs, reason)
    } else if slipping {
        let reason = if signals.repeats + 1 == TOOL_REPETITION_THRESHOLD {
            StepReason::SlippingRepeat
        } else {
            StepReason::SlippingError
        };
        (1, reason)
    } else if routine {
        (-1, StepReason::RoutineReads)
    } else {
        (0, StepReason::Working)
    };
    let conflict = signals.batch == StepBatch::ReadOnly && slipping;
    let ask = if conflict {
        Some(StepAsk::Conflict)
    } else if step > 0 && step.is_multiple_of(STEP_JUDGMENT_EVERY) {
        Some(StepAsk::Cadence)
    } else {
        None
    };
    let judged = judgment.filter(|judgment| !strong && judgment.fresh_at(step));
    let (delta, reason) = match judged.and_then(StepJudgment::delta) {
        Some(delta) if delta != table_delta => (delta, StepReason::Judged),
        _ => (table_delta, reason),
    };
    StepDecision {
        delta,
        reason,
        strong,
        slipping,
        routine,
        ask,
        judged,
    }
}

/// The effort one request carries, as the governor resolved it: the same two
/// fields a wire request reads (`effort`, `effort_band_ceiling`), so a
/// static pin stays a pin and a Smart band stays a band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffortStep {
    pub effort: EffortLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band_ceiling: Option<EffortLevel>,
}

/// Move a turn's effort by `delta` rungs, both ends of a band alike, inside
/// `[low ..= cap]` — `cap` being the person's own ceiling, which the
/// governor never spends past.
#[must_use]
pub fn shift(
    floor: EffortLevel,
    ceiling: Option<EffortLevel>,
    cap: EffortLevel,
    delta: i8,
) -> EffortStep {
    let top = cap.rung();
    let moved = |level: EffortLevel| {
        let rung = i64::try_from(level.rung()).unwrap_or(0) + i64::from(delta);
        let rung = rung.clamp(0, i64::try_from(top).unwrap_or(0));
        EffortLevel::LADDER[usize::try_from(rung).unwrap_or(0)]
    };
    EffortStep {
        effort: moved(floor),
        band_ceiling: ceiling.map(moved),
    }
}

/// Which rung a model move goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RungMove {
    /// The same provider's model a band above.
    Heavier,
    /// The same provider's model a rung below (the catalog's `demotes_to`).
    Lighter,
    /// Another provider's top model — recorded, never applied mid-turn (no
    /// in-turn road exists that does not also arm a quota cooldown).
    CrossTop,
}

impl RungMove {
    /// The word a ledger row carries.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Heavier => "heavier",
            Self::Lighter => "lighter",
            Self::CrossTop => "cross_top",
        }
    }
}

/// A model move the governor decided on, as its row records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepMove {
    pub kind: RungMove,
    pub to: String,
    pub applied: bool,
    /// Why an applying governor did not apply it, when it did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<&'static str>,
}

/// The reason a cross-provider move is recorded and not made.
pub const NO_IN_TURN_ROAD: &str = "no_in_turn_road";

/// The judgment a row consulted, as the row spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepJudgmentRow {
    pub complexity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<i8>,
    pub at_step: u32,
}

/// One decision, as the observer is told it: the request that follows this
/// row is the one it describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRow {
    pub kind: &'static str,
    pub at: u64,
    pub attempt: String,
    pub step: u32,
    /// The chat model the request that follows goes out on. Filed as
    /// `stepModel`, never `model`: that key is the Jev version that answered
    /// a row of the same ledger, and read as one, each step's model cut the
    /// seat's marks away (`zerocode_core::jev::summary::STEP_MODEL`, t-6284).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_model: Option<String>,
    pub band: &'static str,
    pub batch: &'static str,
    pub repeats: usize,
    pub error_streak: u32,
    pub check_red: bool,
    pub routine_streak: u32,
    pub delta: i8,
    pub effort_before: &'static str,
    pub effort_after: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceiling_before: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceiling_after: Option<&'static str>,
    pub reason: &'static str,
    /// Whether the request that follows carries `effort_after`.
    pub applied: bool,
    /// Why it does not, when the host held an applying word back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jev: Option<StepJudgmentRow>,
    #[serde(rename = "move", skip_serializing_if = "Option::is_none")]
    pub rung_move: Option<StepMove>,
}

/// What the observer is told: a decision, or the progress mark written one
/// batch after a judgment was consulted — the seat's own `agreed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum StepEvent {
    /// Boxed: a decision row is a few hundred bytes and a progress mark a
    /// few dozen, and the event is passed by reference either way.
    Step(Box<StepRow>),
    Label(StepLabel),
}

/// The mark one step after a judgment was consulted: whether that step made
/// progress — its batch neither repeated a call, nor errored, nor turned a
/// check red — graded only where the judgment's answer had an effect to
/// grade (`zerocode_core::step_effort::move_mark`, t-6342). Where it had
/// none, the row names why under the summary's `notCompared` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepLabel {
    pub kind: &'static str,
    pub at: u64,
    /// The judgment this mark grades, as the judge joins the two
    /// (`zerocode_core::jev::JevUse::request_name`, t-6877): the turn's
    /// attempt and the step the judgment was ASKED at — the seat's own row
    /// carries both — joined by `:`. A mark that named only the step it was
    /// consulted at named a step the seat was not asked at.
    pub label: String,
    pub attempt: String,
    /// The step the judgment was consulted at.
    pub step: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agreed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_compared: Option<&'static str>,
    /// The table's own move, carried: the seat's baseline, marked by the same
    /// next step (`zerocode_core::jev::summary::BASELINE_AGREED`, t-6342).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_agreed: Option<bool>,
}

/// The word a decision row carries as its kind.
pub const STEP_ROW_KIND: &str = "step";
/// The word a progress mark carries as its kind.
pub const LABEL_ROW_KIND: &str = "label";

/// Told every decision the governor makes. Installed by the host that files
/// the step ledger; the runtime records nothing itself.
pub type StepEffortObserver = Arc<dyn Fn(&StepEvent) + Send + Sync>;

/// What the seat is asked about one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepAskContext<'a> {
    pub step: u32,
    pub why: StepAsk,
    /// The seat's state for the step ([`step_state`]): the turn's own words
    /// — what the routing judgment reads — and the step's counts.
    pub state: &'a Value,
    pub attempt: &'a str,
}

// ---- the seat's question (t-10010) -----------------------------------------

/// The version of the words the seat asks — [`step_questions`] of a
/// [`step_state`] — which every judgment row names: the use table's own
/// number for the seat, read from there and not respelled.
pub const STEP_RUBRIC_VERSION: u32 = zerocode_core::jev::questions::ZO_STEP_EFFORT_RUBRIC_VERSION;

/// The keys of the seat's state: the turn's words, under the routing seat's
/// own key; the step; and the step's counts.
const STEP_STATE_KEYS: [&str; 3] = [ROUTING_STATE_TASK, "step", "signals"];

/// The keys of `signals`: the table's own counts of the batch the step ran.
const STEP_SIGNAL_KEYS: [&str; 4] = ["batch", "repeats", "errors_in_a_row", "check_red"];

/// What the seat asks of the band — the one answer the governor reads —
/// whole, and naming every key of the state (t-10010). Version 1 asked the
/// chat probe's fragment, "how much reasoning/context the task needs end to
/// end", of a string whose line of counts no question mentioned.
const STEP_COMPLEXITY_QUESTION: &str = "How much work does `task` need from start to finish? `step` is how many steps the turn has taken, and `signals` says how its latest step went: `batch` is the kind of tool calls that step made, `repeats` the most times one of those calls has been made this turn, `errors_in_a_row` how many steps in a row had a tool call fail, and `check_red` whether a test or check command failed.";

const _: () = assert!(
    ROUTING_COMPLEXITY_LEVELS.len() == COMPLEXITY_AXIS.tokens.len()
        && ROUTING_RISK_LEVELS.len() == RISK_AXIS.tokens.len(),
    "a routing level stands at each of the router's tokens"
);

/// The seat's state for one step: the turn's words under the routing seat's
/// key — the door cuts them there, by that seat's cap — and the step's
/// counts as fields of their own, which no cut of the words can take
/// (t-10010: version 1 wrote them as a line after the words, and a turn
/// whose words ran past the cap sent no counts at all).
#[must_use]
pub fn step_state(task: &str, step: u32, signals: &StepSignals) -> Value {
    json!({
        STEP_STATE_KEYS[0]: task,
        STEP_STATE_KEYS[1]: step,
        STEP_STATE_KEYS[2]: {
            STEP_SIGNAL_KEYS[0]: signals.batch.label(),
            STEP_SIGNAL_KEYS[1]: signals.repeats,
            STEP_SIGNAL_KEYS[2]: signals.error_streak,
            STEP_SIGNAL_KEYS[3]: signals.check_red,
        },
    })
}

/// The seat's questions, built once: the router's three judged axes asked as
/// Choices over the router's own tokens — what the decision reader
/// (`validate_decision`) reads — and every option in words. Complexity's
/// and risk's are the routing seat's own levels, a token and a level at the
/// same place on the same scale as that seat's reading already takes them;
/// intent's are the router's own descriptions.
#[must_use]
pub fn step_questions() -> &'static BTreeMap<String, SystemOneQuestion> {
    static QUESTIONS: OnceLock<BTreeMap<String, SystemOneQuestion>> = OnceLock::new();
    QUESTIONS.get_or_init(|| {
        let leveled = |axis: &RubricAxis, instructions: &str, levels: &[&'static str]| {
            let options = axis.tokens.iter().zip(levels).map(|(token, level)| (*token, Some(*level)));
            (axis.name.to_string(), SystemOneQuestion::choice(instructions, options))
        };
        let intents = INTENT_AXIS.tokens.iter().map(|token| (*token, INTENT_AXIS.description(token)));
        BTreeMap::from([
            leveled(&COMPLEXITY_AXIS, STEP_COMPLEXITY_QUESTION, &ROUTING_COMPLEXITY_LEVELS),
            leveled(&RISK_AXIS, ROUTING_RISK_QUESTION, &ROUTING_RISK_LEVELS),
            (INTENT_AXIS.name.to_string(), SystemOneQuestion::choice(ROUTING_INTENT_QUESTION, intents)),
        ])
    })
}

/// The seat's request for one step's state.
#[must_use]
pub fn step_request<'a>(model: &'a str, state: &'a Value) -> SystemOneRequest<'a, Value> {
    SystemOneRequest {
        state,
        model,
        questions: step_questions(),
    }
}

/// A seat beside the governor: asked, detached, about a step; read at the
/// next step for whatever answer has arrived since. A seat with nothing to
/// say hands back `None`, and the table's own reading stands.
pub trait StepEffortSeat: Send + Sync {
    fn ask(&self, ask: &StepAskContext<'_>);
    fn take(&self) -> Option<StepJudgment>;
}

/// Everything the host installs for one turn.
#[derive(Clone)]
pub struct StepEffortConfig {
    /// Whether the requests carry the governor's effort (and the model
    /// moves are made): the person's `on`, or an `auto` its evidence raised.
    pub applies: bool,
    /// The turn's effort floor, as the person's `--effort` set it.
    pub floor: EffortLevel,
    /// The band ceiling under Smart, `None` for a static pin.
    pub ceiling: Option<EffortLevel>,
    pub band: RouteTaskComplexity,
    /// The same provider's model a band above the wire model, if any.
    pub heavier_model: Option<String>,
    /// The same provider's rung below the wire model, if any.
    pub lighter_model: Option<String>,
    /// Another provider's top model, if any — recorded only.
    pub cross_top_model: Option<String>,
    pub seat: Option<Arc<dyn StepEffortSeat>>,
    pub observer: Option<StepEffortObserver>,
    /// Why the host holds an applying word back on this wire, when it does
    /// — written on every row, so a ledger that says `applied: false` under
    /// `on` also says why. `None` when nothing is held.
    pub held: Option<&'static str>,
}

impl std::fmt::Debug for StepEffortConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepEffortConfig")
            .field("applies", &self.applies)
            .field("floor", &self.floor)
            .field("ceiling", &self.ceiling)
            .field("band", &self.band)
            .field("heavier_model", &self.heavier_model)
            .field("lighter_model", &self.lighter_model)
            .field("cross_top_model", &self.cross_top_model)
            .field("seat", &self.seat.is_some())
            .field("observer", &self.observer.is_some())
            .field("held", &self.held)
            .finish()
    }
}

impl StepEffortConfig {
    /// The person's own ceiling: the band's top, or the pin itself.
    #[must_use]
    pub fn cap(&self) -> EffortLevel {
        self.ceiling.unwrap_or(self.floor)
    }
}

/// What one batch's results said, folded call by call. Each flag is one
/// independent fact a result may carry, read together once at the end.
#[derive(Debug, Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
struct BatchSeen {
    calls: u32,
    read_only: bool,
    edited: bool,
    checked: bool,
    exec: bool,
    errored: bool,
    check_red: bool,
    repeats: usize,
}

impl BatchSeen {
    fn kind(self) -> StepBatch {
        if self.calls == 0 {
            StepBatch::None
        } else if self.checked {
            StepBatch::Check
        } else if self.edited {
            StepBatch::Edit
        } else if self.exec {
            StepBatch::Exec
        } else if self.read_only {
            StepBatch::ReadOnly
        } else {
            StepBatch::Other
        }
    }
}

/// A judged step awaiting the mark the next step writes, with the two facts
/// that decide whether the mark can say anything about the judgment
/// (`zerocode_core::step_effort::move_mark`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LabelDue {
    step: u32,
    /// The step the judgment consulted at `step` was asked at — the row it
    /// grades.
    asked_at_step: u32,
    /// The request's effort is not the one the table alone would have given
    /// it: the judgment moved it.
    seat_moved_it: bool,
    /// The request carried the governor's effort at all — the seat acts, and
    /// the host held nothing back on this wire.
    carried: bool,
}

/// The governor's state for one turn.
#[derive(Debug)]
pub(super) struct StepEffortState {
    config: StepEffortConfig,
    /// Requests this turn the governor has decided for.
    step: u32,
    batch: BatchSeen,
    error_streak: u32,
    routine_streak: u32,
    strong_streak: u32,
    judgment: Option<StepJudgment>,
    /// The step a fresh judgment was consulted at, awaiting its progress mark.
    label_due: Option<LabelDue>,
    /// The effort the next request carries, when the governor applies.
    next: Option<EffortStep>,
    /// The model the governor moved the turn onto, if it did.
    moved_to: Option<String>,
}

impl StepEffortState {
    fn new(config: StepEffortConfig) -> Self {
        Self {
            config,
            step: 0,
            batch: BatchSeen::default(),
            error_streak: 0,
            routine_streak: 0,
            strong_streak: 0,
            judgment: None,
            label_due: None,
            next: None,
            moved_to: None,
        }
    }

    fn reset_for_turn(&mut self) {
        self.step = 0;
        self.batch = BatchSeen::default();
        self.error_streak = 0;
        self.routine_streak = 0;
        self.strong_streak = 0;
        self.judgment = None;
        self.label_due = None;
        self.next = None;
        self.moved_to = None;
    }
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// The shell tool's name, as its results name it.
const BASH_TOOL: &str = "bash";

/// The command a bash tool input carries, for the check-shaped test.
fn bash_command(input: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(input)
        .ok()?
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Install (or clear) the governor for the turns to come. Host-managed
    /// set-or-clear at every turn entry, like the other per-turn routes.
    pub fn set_step_effort(&mut self, config: Option<StepEffortConfig>) {
        self.step_effort = config.map(StepEffortState::new);
    }

    /// Whether a governor is installed at all.
    #[must_use]
    pub fn step_effort_installed(&self) -> bool {
        self.step_effort.is_some()
    }

    /// Whether the governor is standing aside: a deep-gate PLAN/VERIFY/EXEC
    /// leg or a quota-fallback turn runs on a client of its own, with an
    /// effort of its own, and the governor's reading of the main loop's
    /// steps says nothing about that wire — the same suppression the
    /// per-turn model override keeps in `assemble_request`.
    fn step_effort_stands_aside(&self) -> bool {
        self.deep_leg_owns_the_wire() || self.cross_fallback_active()
    }

    /// The effort the next request carries, when the governor applies —
    /// `None` leaves the client's own effort on the wire.
    pub(super) fn step_effort_for_request(&self) -> Option<EffortStep> {
        if self.step_effort_stands_aside() {
            return None;
        }
        self.step_effort
            .as_ref()
            .filter(|state| state.config.applies)
            .and_then(|state| state.next)
    }

    /// Start a turn: the counters are the turn's own, and the first request
    /// goes out on the turn's effort — there is no batch to read yet. An
    /// internal subturn is a leg of the same turn (a deep-gate PLAN or
    /// VERIFY, an auto-continuation) and keeps the turn's counters.
    pub(super) fn begin_step_effort_turn(&mut self, internal_subturn: bool) {
        if internal_subturn {
            return;
        }
        let Some(state) = self.step_effort.as_mut() else {
            return;
        };
        state.reset_for_turn();
        self.decide_next_step();
    }

    /// Fold one settled tool result into the batch the governor is reading.
    pub(super) fn note_step_tool_result(&mut self, result: &ConversationMessage, tool_input: &str) {
        if self.step_effort_stands_aside() {
            return;
        }
        let Some(state) = self.step_effort.as_mut() else {
            return;
        };
        for block in &result.blocks {
            let ContentBlock::ToolResult {
                tool_name,
                output,
                is_error,
                ..
            } = block
            else {
                continue;
            };
            let seen = &mut state.batch;
            let first = seen.calls == 0;
            seen.calls += 1;
            let read_only = is_concurrency_safe(tool_name);
            seen.read_only = read_only && (first || seen.read_only);
            seen.errored |= *is_error;
            if super::is_edit_or_write_tool(tool_name) && !*is_error {
                seen.edited = true;
            }
            if tool_name == BASH_TOOL {
                let check = bash_command(tool_input).is_some_and(|command| command_is_check_shaped(&command));
                if check {
                    seen.checked = true;
                    seen.check_red |= *is_error || !bash_result_exited_zero(output);
                } else {
                    seen.exec = true;
                }
            }
            let count = self
                .tool_fingerprint_counts
                .get(&fingerprint_tool_call(tool_name, tool_input))
                .copied()
                .unwrap_or(1);
            seen.repeats = seen.repeats.max(count);
        }
    }

    /// Read the batch that just finished and decide the next request's
    /// effort: the signals to the table, the table to a rung, the rung to
    /// the request; the seat asked when the table says so; the model moved
    /// when the strong signals say so and the governor applies.
    pub(super) fn govern_step_after_batch(&mut self) {
        if self.step_effort.is_none() || self.step_effort_stands_aside() {
            return;
        }
        self.decide_next_step();
    }

    fn decide_next_step(&mut self) {
        // The wire model and the attempt are read before the state is
        // borrowed: the row names both, and the state is what decides.
        let wire = self.wire_model().map(|(model, _)| model.to_string());
        let attempt = self.attempt.clone();
        let Some(state) = self.step_effort.as_mut() else {
            return;
        };
        let planned = state.plan(wire.as_deref(), &attempt);
        let observer = state.config.observer.clone();
        let seat = state.config.seat.clone();
        self.apply_step_move(wire.as_deref(), planned.moved_to, planned.return_home);
        if let Some(observer) = observer.as_ref() {
            if let Some(label) = planned.label {
                observer(&StepEvent::Label(label));
            }
            observer(&StepEvent::Step(Box::new(planned.row)));
        }
        if let (Some(seat), Some(why)) = (seat, planned.ask) {
            let words = self
                .latest_user_text()
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default();
            let state = step_state(&words, planned.step, &planned.signals);
            seat.ask(&StepAskContext {
                step: planned.step,
                why,
                state: &state,
                attempt: &attempt,
            });
        }
    }

    /// Move the wire model through the runtime's own same-provider override
    /// — set without the freshness latch, so the next turn's begin clears it
    /// as it clears any override nobody freshly installed — and tell the
    /// switch observer, as every other door does.
    fn apply_step_move(&mut self, wire: Option<&str>, moved_to: Option<String>, return_home: bool) {
        if let Some(to) = moved_to {
            if let Some(from) = wire {
                self.note_model_switch(SwitchTrigger::Step, from, &to);
            }
            self.escalation_model_override = Some(to);
        } else if return_home {
            let home = self.context_model.clone();
            if let (Some(from), Some(home)) = (wire, home.as_deref()) {
                self.note_model_switch(SwitchTrigger::Step, from, home);
            }
            self.escalation_model_override = None;
        }
    }
}

/// One step's plan, as the state decided it: the row and the mark to file,
/// the model to move to (or to leave), and the question to ask.
struct PlannedStep {
    step: u32,
    row: StepRow,
    label: Option<StepLabel>,
    /// A model the governor moves onto now — `None` when it stays, or when
    /// it already stands on this one.
    moved_to: Option<String>,
    /// Whether a lighter model the governor moved onto is left for the
    /// session's own.
    return_home: bool,
    ask: Option<StepAsk>,
    /// What the table read at the step — the counts the seat's state carries.
    signals: StepSignals,
}

impl StepEffortState {
    /// The progress mark for the judgment consulted one step ago, now that
    /// this step says whether it `progressed` (t-6342).
    fn label_owed(&mut self, attempt: &str, progressed: bool) -> Option<StepLabel> {
        self.label_due.take().map(|due| {
            let graded = zerocode_core::step_effort::move_mark(due.seat_moved_it, due.carried, progressed);
            StepLabel {
                kind: LABEL_ROW_KIND,
                at: unix_millis(),
                // Spelled as the seat's judgment row spells its attempt
                // (trimmed) and its step.
                label: format!("{}:{}", attempt.trim(), due.asked_at_step),
                attempt: attempt.to_string(),
                step: due.step,
                agreed: graded.ok(),
                not_compared: graded.err(),
                baseline_agreed: (due.carried && !due.seat_moved_it).then_some(progressed),
            }
        })
    }

    /// Owe the next step a label for the judgment that spoke to this one:
    /// whether it moved the effort away from what the table alone would have
    /// given the request (`shifted` against the table's own), and whether
    /// the request carried it.
    fn owe_label(&mut self, step: u32, asked_at_step: u32, signals: &StepSignals, shifted: EffortStep) {
        let table = decide(step, signals, None);
        let ruled = shift(self.config.floor, self.config.ceiling, self.config.cap(), table.delta);
        self.label_due = Some(LabelDue {
            step,
            asked_at_step,
            seat_moved_it: shifted != ruled,
            carried: self.config.applies,
        });
    }

    /// Read the batch that just finished and decide the next request: the
    /// signals to the table, the table to a rung, the rung to the request.
    fn plan(&mut self, wire: Option<&str>, attempt: &str) -> PlannedStep {
        self.step += 1;
        let step = self.step;
        let batch = std::mem::take(&mut self.batch);
        let kind = batch.kind();
        if batch.calls > 0 {
            self.error_streak = if batch.errored { self.error_streak + 1 } else { 0 };
            self.routine_streak = if kind == StepBatch::ReadOnly { self.routine_streak + 1 } else { 0 };
        }
        // A judgment that arrived since the last step speaks from this one.
        if let Some(answer) = self.config.seat.as_ref().and_then(|seat| seat.take()) {
            self.judgment = Some(answer);
        }
        let signals = StepSignals {
            batch: kind,
            repeats: batch.repeats.max(usize::from(batch.calls > 0)),
            error_streak: self.error_streak,
            check_red: batch.check_red,
            routine_streak: self.routine_streak,
            band: self.config.band,
        };
        let decision = decide(step, &signals, self.judgment);
        self.strong_streak = if decision.strong { self.strong_streak + 1 } else { 0 };
        let progressed = batch.calls > 0 && !decision.strong && !decision.slipping && !batch.check_red;
        let label = self.label_owed(attempt, progressed);
        let shifted = shift(self.config.floor, self.config.ceiling, self.config.cap(), decision.delta);
        if let Some(judged) = decision.judged {
            self.owe_label(step, judged.at_step, &signals, shifted);
        }
        self.next = Some(shifted);
        let rung_move = plan_move(&self.config, &decision, self.strong_streak, self.routine_streak, shifted);
        let applied = self.config.applies;
        let (planned_model, move_row) = match rung_move {
            Some((kind, to)) => {
                let can_apply = applied && kind != RungMove::CrossTop;
                let why = (applied && !can_apply).then_some(NO_IN_TURN_ROAD);
                (can_apply.then(|| to.clone()), Some(StepMove { kind, to, applied: can_apply, why }))
            }
            None => (None, None),
        };
        // Moving onto a model the turn already stands on is no move; leaving
        // a lighter model is the first stuck or slipping step after it.
        let previously = self.moved_to.clone();
        let stays_put = planned_model.is_none();
        let moved_to = planned_model.filter(|to| previously.as_deref() != Some(to.as_str()));
        let return_home = previously.is_some() && stays_put && (decision.strong || decision.slipping);
        if moved_to.is_some() {
            self.moved_to.clone_from(&moved_to);
        } else if return_home {
            self.moved_to = None;
        }
        let row = StepRow {
            kind: STEP_ROW_KIND,
            at: unix_millis(),
            attempt: attempt.to_string(),
            step,
            step_model: wire.map(str::to_string),
            band: signals.band.as_label(),
            batch: kind.label(),
            repeats: signals.repeats,
            error_streak: signals.error_streak,
            check_red: signals.check_red,
            routine_streak: signals.routine_streak,
            delta: decision.delta,
            effort_before: self.config.floor.label(),
            effort_after: shifted.effort.label(),
            ceiling_before: self.config.ceiling.map(EffortLevel::label),
            ceiling_after: shifted.band_ceiling.map(EffortLevel::label),
            reason: decision.reason.token(),
            applied,
            held: self.config.held,
            ask: decision.ask.filter(|_| self.config.seat.is_some()).map(StepAsk::token),
            jev: decision.judged.map(|judged| StepJudgmentRow {
                complexity: judged.complexity.as_label(),
                delta: judged.delta(),
                at_step: judged.at_step,
            }),
            rung_move: move_row,
        };
        PlannedStep {
            step,
            row,
            label,
            moved_to,
            return_home,
            ask: decision.ask.filter(|_| self.config.seat.is_some()),
            signals,
        }
    }
}

/// Whether the strong or the routine signals have gone on long enough, with
/// the effort already at the end of its ladder, to move the model a rung.
fn plan_move(
    config: &StepEffortConfig,
    decision: &StepDecision,
    strong_streak: u32,
    routine_streak: u32,
    shifted: EffortStep,
) -> Option<(RungMove, String)> {
    let at_cap = shifted.effort.rung() >= config.cap().rung();
    if decision.strong && strong_streak >= STRONG_STEPS_FOR_HEAVIER && at_cap {
        return config
            .heavier_model
            .clone()
            .map(|model| (RungMove::Heavier, model))
            .or_else(|| config.cross_top_model.clone().map(|model| (RungMove::CrossTop, model)));
    }
    let on_floor = shifted.effort == EffortLevel::LADDER[0];
    if decision.routine && routine_streak >= ROUTINE_STEPS_FOR_LIGHTER && on_floor {
        return config
            .lighter_model
            .clone()
            .map(|model| (RungMove::Lighter, model));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(batch: StepBatch, repeats: usize, error_streak: u32, check_red: bool, routine_streak: u32) -> StepSignals {
        StepSignals {
            batch,
            repeats,
            error_streak,
            check_red,
            routine_streak,
            band: RouteTaskComplexity::Medium,
        }
    }

    #[test]
    fn a_routine_read_batch_goes_one_rung_down_and_work_stays() {
        let routine = decide(2, &signals(StepBatch::ReadOnly, 1, 0, false, 1), None);
        assert_eq!((routine.delta, routine.reason), (-1, StepReason::RoutineReads));
        assert!(routine.routine && !routine.strong && !routine.slipping);
        for batch in [StepBatch::Edit, StepBatch::Exec, StepBatch::Check, StepBatch::Other] {
            let working = decide(2, &signals(batch, 1, 0, false, 0), None);
            assert_eq!((working.delta, working.reason), (0, StepReason::Working), "{batch:?}");
        }
    }

    #[test]
    fn stuck_signals_climb_and_the_easy_bands_climb_less() {
        let red = decide(3, &signals(StepBatch::Check, 1, 0, true, 0), None);
        assert_eq!((red.delta, red.reason), (2, StepReason::StuckCheckRed));
        let repeat = decide(3, &signals(StepBatch::ReadOnly, TOOL_REPETITION_THRESHOLD, 0, false, 3), None);
        assert_eq!((repeat.delta, repeat.reason), (2, StepReason::StuckRepeat));
        assert!(repeat.strong && !repeat.routine);
        let errors = decide(3, &signals(StepBatch::Exec, 1, STUCK_ERROR_STREAK, false, 0), None);
        assert_eq!((errors.delta, errors.reason), (2, StepReason::StuckErrors));
        let mut easy = signals(StepBatch::Check, 1, 0, true, 0);
        easy.band = RouteTaskComplexity::Small;
        assert_eq!(decide(3, &easy, None).delta, 1);
    }

    #[test]
    fn slipping_signals_climb_one_rung_and_a_slipping_read_is_a_conflict() {
        let second_call = decide(2, &signals(StepBatch::ReadOnly, TOOL_REPETITION_THRESHOLD - 1, 0, false, 2), None);
        assert_eq!((second_call.delta, second_call.reason), (1, StepReason::SlippingRepeat));
        assert_eq!(second_call.ask, Some(StepAsk::Conflict));
        let one_error = decide(2, &signals(StepBatch::Edit, 1, 1, false, 0), None);
        assert_eq!((one_error.delta, one_error.reason), (1, StepReason::SlippingError));
        assert_eq!(one_error.ask, None);
    }

    #[test]
    fn the_seat_is_asked_on_the_cadence_and_its_fresh_band_breaks_a_weak_reading() {
        let cadence = decide(STEP_JUDGMENT_EVERY, &signals(StepBatch::Edit, 1, 0, false, 0), None);
        assert_eq!(cadence.ask, Some(StepAsk::Cadence));
        assert_eq!(decide(STEP_JUDGMENT_EVERY + 1, &signals(StepBatch::Edit, 1, 0, false, 0), None).ask, None);
        let large = StepJudgment {
            complexity: RouteTaskComplexity::Large,
            at_step: STEP_JUDGMENT_EVERY,
        };
        let judged = decide(STEP_JUDGMENT_EVERY + 1, &signals(StepBatch::ReadOnly, 1, 0, false, 1), Some(large));
        assert_eq!((judged.delta, judged.reason), (1, StepReason::Judged));
        assert_eq!(judged.judged, Some(large));
        // Not from the step it was asked at, and not past a cadence.
        assert_eq!(decide(STEP_JUDGMENT_EVERY, &signals(StepBatch::ReadOnly, 1, 0, false, 1), Some(large)).judged, None);
        assert_eq!(
            decide(2 * STEP_JUDGMENT_EVERY + 1, &signals(StepBatch::ReadOnly, 1, 0, false, 1), Some(large)).judged,
            None
        );
        // A strong signal outranks it.
        let stuck = decide(STEP_JUDGMENT_EVERY + 1, &signals(StepBatch::Check, 1, 0, true, 0), Some(large));
        assert_eq!((stuck.delta, stuck.judged), (2, None));
        // An agreeing band keeps the table's own word.
        let medium = StepJudgment {
            complexity: RouteTaskComplexity::Medium,
            at_step: STEP_JUDGMENT_EVERY,
        };
        let same = decide(STEP_JUDGMENT_EVERY + 1, &signals(StepBatch::Edit, 1, 0, false, 0), Some(medium));
        assert_eq!((same.delta, same.reason), (0, StepReason::Working));
        assert_eq!(same.judged, Some(medium));
        let unknown = StepJudgment {
            complexity: RouteTaskComplexity::Unknown,
            at_step: STEP_JUDGMENT_EVERY,
        };
        assert_eq!(unknown.delta(), None);
    }

    #[test]
    fn a_large_turn_needs_two_reads_before_a_step_is_routine() {
        let mut first = signals(StepBatch::ReadOnly, 1, 0, false, 1);
        first.band = RouteTaskComplexity::Large;
        assert_eq!(decide(2, &first, None).reason, StepReason::Working);
        first.routine_streak = LARGE_ROUTINE_STEPS;
        assert_eq!(decide(3, &first, None).reason, StepReason::RoutineReads);
    }

    #[test]
    fn a_shift_moves_both_ends_of_a_band_inside_the_persons_cap() {
        use EffortLevel as L;
        // Smart: [xhigh .. max], cap max.
        assert_eq!(
            shift(L::Xhigh, Some(L::Max), L::Max, -1),
            EffortStep { effort: L::High, band_ceiling: Some(L::Xhigh) }
        );
        assert_eq!(
            shift(L::Xhigh, Some(L::Max), L::Max, 2),
            EffortStep { effort: L::Max, band_ceiling: Some(L::Max) }
        );
        // A static pin only ever goes down: the pin is the cap.
        assert_eq!(shift(L::High, None, L::High, 2), EffortStep { effort: L::High, band_ceiling: None });
        assert_eq!(shift(L::High, None, L::High, -1), EffortStep { effort: L::Medium, band_ceiling: None });
        // Never under the ladder's floor.
        assert_eq!(shift(L::Low, None, L::Low, -1), EffortStep { effort: L::Low, band_ceiling: None });
    }

    fn config(applies: bool) -> StepEffortConfig {
        StepEffortConfig {
            applies,
            floor: EffortLevel::Xhigh,
            ceiling: Some(EffortLevel::Max),
            band: RouteTaskComplexity::Medium,
            heavier_model: Some("heavier".to_string()),
            lighter_model: Some("lighter".to_string()),
            cross_top_model: Some("other-top".to_string()),
            seat: None,
            observer: None,
            held: None,
        }
    }

    #[test]
    fn a_model_moves_only_on_a_second_strong_step_at_the_cap_or_a_fifth_routine_step_on_the_floor() {
        let config = config(true);
        let stuck = decide(3, &signals(StepBatch::Check, 1, 0, true, 0), None);
        let at_cap = shift(config.floor, config.ceiling, config.cap(), stuck.delta);
        assert_eq!(plan_move(&config, &stuck, 1, 0, at_cap), None, "one strong step is a rung of effort");
        assert_eq!(
            plan_move(&config, &stuck, STRONG_STEPS_FOR_HEAVIER, 0, at_cap),
            Some((RungMove::Heavier, "heavier".to_string()))
        );
        let mut no_heavier = config.clone();
        no_heavier.heavier_model = None;
        assert_eq!(
            plan_move(&no_heavier, &stuck, STRONG_STEPS_FOR_HEAVIER, 0, at_cap),
            Some((RungMove::CrossTop, "other-top".to_string()))
        );
        let routine = decide(6, &signals(StepBatch::ReadOnly, 1, 0, false, ROUTINE_STEPS_FOR_LIGHTER), None);
        let one_down = shift(config.floor, config.ceiling, config.cap(), routine.delta);
        assert_eq!(plan_move(&config, &routine, 0, ROUTINE_STEPS_FOR_LIGHTER, one_down), None, "high is not the floor");
        let on_floor = EffortStep { effort: EffortLevel::Low, band_ceiling: None };
        assert_eq!(
            plan_move(&config, &routine, 0, ROUTINE_STEPS_FOR_LIGHTER, on_floor),
            Some((RungMove::Lighter, "lighter".to_string()))
        );
        assert_eq!(plan_move(&config, &routine, 0, ROUTINE_STEPS_FOR_LIGHTER - 1, on_floor), None);
    }

    #[test]
    fn rows_spell_their_kind_and_serialize_the_move_under_its_own_word() {
        let row = StepRow {
            kind: STEP_ROW_KIND,
            at: 1,
            attempt: "s@1".to_string(),
            step: 2,
            step_model: Some("m".to_string()),
            band: "medium",
            batch: "read_only",
            repeats: 1,
            error_streak: 0,
            check_red: false,
            routine_streak: 1,
            delta: -1,
            effort_before: "xhigh",
            effort_after: "high",
            ceiling_before: Some("max"),
            ceiling_after: Some("xhigh"),
            reason: "routine_reads",
            applied: false,
            held: None,
            ask: None,
            jev: None,
            rung_move: Some(StepMove {
                kind: RungMove::CrossTop,
                to: "other".to_string(),
                applied: false,
                why: Some(NO_IN_TURN_ROAD),
            }),
        };
        let value = serde_json::to_value(StepEvent::Step(Box::new(row))).expect("row");
        assert_eq!(value["kind"], "step");
        assert_eq!(value["move"]["kind"], "cross_top");
        assert_eq!(value["move"]["why"], NO_IN_TURN_ROAD);
        assert_eq!(value["errorStreak"], 0);
        assert!(value.get("outcome").is_none(), "a decision is not a request the seat answered");
        // The chat model the step ran on has a key of its own: `model` is the
        // version that answered a Jev row, and read as one, each step cut the
        // seat's marks away (t-6284).
        assert_eq!(value[zerocode_core::jev::summary::STEP_MODEL.canonical], "m");
        assert!(zerocode_core::jev::summary::MODEL.read(&value).is_none(), "a step names no version: {value}");
        let label = serde_json::to_value(StepEvent::Label(StepLabel {
            kind: LABEL_ROW_KIND,
            at: 2,
            label: "s@1:1".to_string(),
            attempt: "s@1".to_string(),
            step: 2,
            agreed: Some(true),
            not_compared: None,
            baseline_agreed: None,
        }))
        .expect("label");
        assert_eq!(label["kind"], "label");
        assert_eq!(label["label"], "s@1:1", "the mark names the judgment it grades");
        assert_eq!(label["agreed"], true);
        assert!(label.get("notCompared").is_none(), "{label}");
    }

    /// A seat that hands back the answer it was built with, once.
    struct Answering(std::sync::Mutex<Option<StepJudgment>>);

    impl StepEffortSeat for Answering {
        fn ask(&self, _ask: &StepAskContext<'_>) {}
        fn take(&self) -> Option<StepJudgment> {
            self.0.lock().ok().and_then(|mut held| held.take())
        }
    }

    /// The turn's steps after a judgment answered at step 1 that the task is
    /// `complexity`: a routine read (the table would lower the effort), then
    /// a clean edit — the step the mark grades. Answers the one label filed.
    fn graded_after(complexity: RouteTaskComplexity, applies: bool, held: Option<&'static str>) -> StepLabel {
        let mut config = config(applies);
        config.held = held;
        config.seat = Some(Arc::new(Answering(std::sync::Mutex::new(Some(StepJudgment {
            complexity,
            at_step: 1,
        })))));
        let mut state = StepEffortState::new(config);
        state.step = 1;
        state.batch = BatchSeen { calls: 1, read_only: true, ..BatchSeen::default() };
        state.routine_streak = 1;
        let judged = state.plan(Some("m"), "s@1");
        assert!(judged.label.is_none(), "nothing was consulted before step 2");
        assert!(judged.row.jev.is_some(), "the step consulted the judgment: {:?}", judged.row);
        state.batch = BatchSeen { calls: 1, edited: true, ..BatchSeen::default() };
        state.plan(Some("m"), "s@1").label.expect("the judged step's mark")
    }

    /// "The next step made progress" is not a label by itself (t-6342): on
    /// this machine's ledger 522 of 547 steps after a judgment progressed
    /// whatever the judgment said, and all 440 judgments that differed from
    /// the table were held back on an Anthropic wire — none of them moved a
    /// request the next step could answer for. The mark is written only where
    /// the judgment changed the effort a request carried.
    #[test]
    fn a_step_is_graded_only_where_the_judgment_moved_what_the_request_carried() {
        use zerocode_core::step_effort::{NOT_CARRIED, SAME_AS_RULE};
        // Held on this wire: the request never carried the move.
        let held = graded_after(RouteTaskComplexity::Large, false, Some("anthropic_cache_prefix"));
        assert_eq!((held.agreed, held.not_compared), (None, Some(NOT_CARRIED)));
        // A recording seat: nothing carried either.
        let recording = graded_after(RouteTaskComplexity::Large, false, None);
        assert_eq!((recording.agreed, recording.not_compared), (None, Some(NOT_CARRIED)));
        // Carried, and the judgment moved the effort (a large task held the
        // rung the table would have lowered): the next step's progress is
        // its grade.
        let moved = graded_after(RouteTaskComplexity::Large, true, None);
        assert_eq!((moved.agreed, moved.not_compared), (Some(true), None));
        // And the mark names the judgment it grades as the seat's judgment
        // row names it — the turn and the step the judgment was ASKED at,
        // not the later step it was consulted at (t-6877).
        assert_eq!((moved.label.as_str(), moved.step), ("s@1:1", 2));
        // Carried, but the judgment said what the table said: nothing to grade.
        let same = graded_after(RouteTaskComplexity::Small, true, None);
        assert_eq!((same.agreed, same.not_compared), (None, Some(SAME_AS_RULE)));
    }

    /// Every word the seat asks, as one string: the questions as they go on
    /// the wire — each one's instructions and every option's words — and the
    /// keys of the state they read.
    fn step_rubric_words() -> String {
        let questions = serde_json::to_string(step_questions()).expect("the questions serialize");
        [questions, STEP_STATE_KEYS.join(","), STEP_SIGNAL_KEYS.join(",")].join("\n")
    }

    /// The seat's words are pinned to its version (t-10010): a question, an
    /// option's words or a key of the state changed without a version is red,
    /// not a quiet drift of the series the seat is judged on.
    #[test]
    fn the_step_version_is_pinned_to_its_words() {
        assert_eq!(STEP_RUBRIC_VERSION, 2);
        assert_eq!(
            zerocode_core::jev::rubric_fingerprint(step_rubric_words),
            "7a54303feca84f7e",
            "{}",
            step_rubric_words()
        );
    }

    /// The seat asks what the decision reader reads — the router's judged
    /// axes, each over its own tokens — and every option says what it means
    /// (t-10010): complexity's and risk's in the routing seat's own levels, a
    /// token and a level at the same place on the same scale as that seat's
    /// reading already takes them; intent's in the router's own words.
    /// Version 1 described two of complexity's four options and none of
    /// risk's.
    #[test]
    fn every_option_the_seat_offers_says_what_it_means() {
        use ::api::{SystemOneCriteria, SystemOneQuestionKind};
        let questions = step_questions();
        let asked: Vec<&str> = questions.keys().map(String::as_str).collect();
        let mut judged: Vec<&str> = crate::model_router::judged_axes().map(|axis| axis.name).collect();
        judged.sort_unstable();
        assert_eq!(asked, judged, "the axes the decision reader reads");
        let options = |axis: &RubricAxis| {
            let question = &questions[axis.name];
            assert_eq!(question.kind, SystemOneQuestionKind::Choice, "{}", axis.name);
            let SystemOneCriteria::Named(criteria) = &question.criteria else {
                panic!("{}: an axis offers named options", axis.name);
            };
            criteria.clone()
        };
        for axis in crate::model_router::judged_axes() {
            let criteria = options(axis);
            let mut offered: Vec<&str> = criteria.keys().map(String::as_str).collect();
            let mut tokens = axis.tokens.to_vec();
            offered.sort_unstable();
            tokens.sort_unstable();
            assert_eq!(offered, tokens, "{}: the router's own tokens", axis.name);
            for (token, means) in &criteria {
                assert!(
                    means.as_deref().is_some_and(|means| !means.trim().is_empty()),
                    "{}/{token} says nothing of what it means",
                    axis.name
                );
            }
        }
        for (axis, levels) in [(&COMPLEXITY_AXIS, ROUTING_COMPLEXITY_LEVELS), (&RISK_AXIS, ROUTING_RISK_LEVELS)] {
            let criteria = options(axis);
            for (token, level) in axis.tokens.iter().zip(levels) {
                assert_eq!(criteria[*token].as_deref(), Some(level), "{}/{token}", axis.name);
            }
        }
    }

    /// The seat asks whole questions of its state (t-10010): every key the
    /// state carries, and every count under `signals`, is named in backticks
    /// by a question — where version 1 asked fragments of the chat probe's
    /// prompt that named nothing the state held.
    #[test]
    fn the_seats_questions_name_every_key_of_its_state() {
        let asked: Vec<&str> = step_questions()
            .values()
            .map(|question| question.instructions.as_str())
            .collect();
        let asked = asked.join("\n");
        for key in STEP_STATE_KEYS.iter().chain(&STEP_SIGNAL_KEYS) {
            assert!(asked.contains(&format!("`{key}`")), "no question names `{key}`:\n{asked}");
        }
    }

    /// The step's counts are fields of the state (t-10010), not a line after
    /// the turn's words: the door cuts the words by their own pointer, and a
    /// turn past the cap keeps its counts.
    #[test]
    fn the_state_carries_the_turns_words_and_the_steps_counts_as_fields() {
        let counts = signals(StepBatch::ReadOnly, 2, 1, false, 3);
        let state = step_state("fix the parser", 5, &counts);
        assert_eq!(
            state,
            serde_json::json!({
                "task": "fix the parser",
                "step": 5,
                "signals": { "batch": "read_only", "repeats": 2, "errors_in_a_row": 1, "check_red": false },
            })
        );
        let mut keys: Vec<&str> = state
            .as_object()
            .map(|fields| fields.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        let mut expected = STEP_STATE_KEYS.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected);
    }
}
