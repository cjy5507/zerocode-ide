//! Whether a worker should go on (t-26583 — ruflo's guidance `ContinueGate`,
//! read against what this window already knows of an attempt).
//!
//! Every gate this window has judges ONE call: the tool guards ask whether this
//! command may run, the quota gate whether this summons may start. A worker that
//! has gone wrong rarely makes one bad call — it repeats a good one, or each
//! call costs a little more than the last, and no single step is the mistake.
//! This is the judgment over the run: four words, in rising order of what they
//! ask of the window.
//!
//! - [`Verdict::Continue`] — nothing to say.
//! - [`Verdict::Checkpoint`] — save the work now: its state is a restore point
//!   before it goes on (every [`CHECKPOINT_EVERY_STEPS`] calls, and when the
//!   cost of its calls is rising).
//! - [`Verdict::Pause`] — somebody should look: it is repeating itself
//!   ([`REWORK_LIMIT_PERMILLE`]), or a budget a person set is nearly spent.
//! - [`Verdict::Stop`] — go no further: the next [`PROJECTION_STEPS`] calls
//!   would spend the budget, so it ends BEFORE the budget does.
//!
//! What it reads is three facts the window holds for every CLI the same way —
//! no CLI is asked anything:
//!
//! - **steps and rework**, from the hook server's tool events
//!   ([`StepBook::note_activity`]): the one envelope every agent's hook becomes
//!   ([`crate::hook::Activity`]). A call is "the same" as the one before it when
//!   its tool and its whole input are — [`crate::step_effort::signals_of`]'s own
//!   notion of a loop, read live instead of at a turn's end. A call that failed
//!   is rework too.
//! - **cost**, from the CLI's own record of its model calls, through one small
//!   reader per transcript format ([`spend`]). A CLI with no reader says so and
//!   is judged on the first fact alone — never on a cost nobody read.
//! - **the budget**, a person's ([`Settings`]), as caps the window fills in
//!   ([`Allowance`]).
//!
//! Nothing here touches a pane, a file, a clock or the network: the time
//! arrives as an argument and every rule below is a test, not a hope.

pub mod spend;

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

// `Phase` is the tests' (they reach it through this module); the stub has no use for it.
#[allow(unused_imports)]
use crate::hook::{Activity, Phase};

/// Tool calls between two forced checkpoints. Two hundred: a worker on this
/// machine makes calls at a rate of the order of a thousand an hour — the
/// activity ring's own comment counts "thousands" — so a checkpoint comes every
/// few minutes of real work, often enough that a lost pane costs minutes and
/// seldom enough that the snapshot (a status and a tree write) is noise.
pub const CHECKPOINT_EVERY_STEPS: u32 = 200;

/// The shortest time between two forced checkpoints: a worker making calls as
/// fast as a hook can report them is not thereby owed a snapshot every few
/// seconds. Five minutes.
pub const CHECKPOINT_MIN_GAP_MS: i64 = 5 * 60 * 1_000;

/// How many of an attempt's latest tool calls the rework ratio is read over.
/// A window and not the attempt's whole life: a worker that did a thousand
/// good calls and then loops must show it within the window, not be diluted
/// by what it did before. Twenty-four, so that the ratio's ceiling
/// ([`REWORK_LIMIT_PERMILLE`]) is crossed by the eighth repeat.
pub const REWORK_WINDOW_STEPS: usize = 24;

/// The fewest calls the window must hold before its ratio is read. Sixteen:
/// five repeats among sixteen calls is a pattern where five among ten may be a
/// retry or two — [`crate::step_effort::REPEATS_THAT_RAISE`] already calls three
/// in a row a loop, and a window of sixteen holds a run of that length and more
/// before it can pass the ceiling.
pub const REWORK_MIN_STEPS: usize = 16;

/// The most rework a window may hold: three calls in ten that redid something
/// or failed. The ceiling the task named, as a ratio.
pub const REWORK_LIMIT_PERMILLE: u32 = 300;

/// How many model calls' costs one attempt keeps, newest last. Sixty-four is
/// five windows of [`RISE_WINDOW`] and more than the projection reads; past it
/// the oldest go, and the attempt's total is a running sum that never needed
/// them.
pub const COST_RING: usize = 64;

/// The model calls each half of the cost-trend compares: the latest six against
/// the six before them. Medians of six step over a lone expensive call — a
/// cache that expired costs one call ten times its neighbours — where a mean
/// would take it for a trend.
pub const RISE_WINDOW: usize = 6;

/// How much dearer the latest calls must be than the ones before for the trend
/// to be called rising: twice, in thousandths. A context that grows by
/// reading doubles over many calls; doubling inside twelve is a flood.
pub const RISE_FACTOR_PERMILLE: u32 = 2_000;

/// The dearest a call can be and still be too cheap to call a trend: below ten
/// cents a call, doubling is a rounding of the vendor's table.
pub const RISE_MIN_USD: f64 = 0.10;

/// How many calls ahead a budget is judged: eight. The window learns a call's
/// cost one call late (the vendor writes it as the call ends), the beat reads
/// it a poll later, and a stop takes a moment to land — at the fastest cadence
/// seen, a call a second, that is as many as eight calls spent after the
/// decision. A stop that waited for the budget to be spent would end the worker
/// after it.
pub const PROJECTION_STEPS: u32 = 8;

/// How many of the latest calls the projection takes the median of, and how
/// many it needs before it projects at all. A median, so that one expensive
/// call — a session's first writes its whole context to the cache — is not
/// eight of them.
pub const PROJECTION_RECENT_CALLS: usize = 5;

/// The fewest calls the projection needs: three. Before them the budget is
/// judged on what is already spent and nothing more.
pub const PROJECTION_MIN_CALLS: usize = 3;

/// The share of a budget, in thousandths, from which a worker is told to be
/// looked at before the stop comes: eighty in a hundred.
pub const BUDGET_NEAR_PERMILLE: u32 = 800;

/// How many bytes of a call's input make its print. A call's identity is its
/// tool and the words of its input; a write's whole file is not words anyone
/// repeats by accident, and hashing megabytes per event is a cost on every
/// agent for nothing.
pub const PRINT_BYTES: usize = 4_096;

/// The largest budget a settings file may name, in dollars: a typo guard. No
/// worker's task costs a hundred thousand dollars; a budget that large is a
/// slipped key and is read as none.
pub const BUDGET_USD_MAX: f64 = 100_000.0;

const _: () = assert!(REWORK_MIN_STEPS <= REWORK_WINDOW_STEPS);
const _: () = assert!(2 * RISE_WINDOW <= COST_RING);
const _: () = assert!(PROJECTION_RECENT_CALLS <= COST_RING);
const _: () = assert!(PROJECTION_MIN_CALLS <= PROJECTION_RECENT_CALLS);

/// What the judgment tells the window to do, in rising order.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Nothing to say.
    #[default]
    Continue,
    /// Save the work: its state is a restore point.
    Checkpoint,
    /// Somebody should look.
    Pause,
    /// Go no further.
    Stop,
}

impl Verdict {
    /// The word a row and a screen key on.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Checkpoint => "checkpoint",
            Self::Pause => "pause",
            Self::Stop => "stop",
        }
    }
}

/// Why the judgment says what it says — one code per fact, so the board can
/// word each in a person's language and carry its numbers beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// `value` calls since the last checkpoint, `limit` between two.
    CheckpointDue,
    /// The latest model calls cost `value` thousandths of the ones before,
    /// against `limit`.
    CostRising,
    /// `value` thousandths of the latest calls redid something or failed,
    /// against `limit`.
    ReworkLoop,
    /// The task's spend, in dollars, is `value` of the `limit` a person gave.
    TaskBudgetNear,
    /// The same for the day's.
    DayBudgetNear,
    /// The task's spend and the next [`PROJECTION_STEPS`] calls would come to
    /// `value` dollars of the `limit`.
    TaskBudgetStop,
    /// The same for the day's.
    DayBudgetStop,
}

impl Code {
    /// The verdict this fact alone earns.
    #[must_use]
    pub const fn verdict(self) -> Verdict {
        match self {
            Self::CheckpointDue | Self::CostRising => Verdict::Checkpoint,
            Self::ReworkLoop | Self::TaskBudgetNear | Self::DayBudgetNear => Verdict::Pause,
            Self::TaskBudgetStop | Self::DayBudgetStop => Verdict::Stop,
        }
    }
}

/// One fact behind a verdict, with the number it was read at and the line it
/// was held to — in the unit its [`Code`] names.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    pub code: Code,
    pub value: f64,
    pub limit: f64,
}

/// One budget a person set, as the window reads it for the worker it judges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cap {
    /// What the person gave, in API-equivalent dollars.
    pub limit_usd: f64,
    /// What the scope has spent so far, as far as the window read it.
    pub spent_usd: f64,
    /// What everything running in the scope is still to spend before a stop
    /// can land: the next [`PROJECTION_STEPS`] calls of each
    /// ([`StepBook::ahead_usd`]).
    pub ahead_usd: f64,
}

/// The budgets that apply to one worker: its task's and the day's. `None` is no
/// budget of that kind — or one the window cannot read, which is the same to
/// the judgment and is said to the person elsewhere.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Allowance {
    pub task: Option<Cap>,
    pub day: Option<Cap>,
}

/// What the window does with a verdict: nothing, tell, or tell and act.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The gate judges nothing: no counts, no checkpoint, no notice.
    Off,
    /// It judges, saves checkpoints, shows the verdict and tells the
    /// coordinator; it ends no worker. The default: a window that stopped
    /// somebody's work on a rule nobody declared would be the waste.
    #[default]
    Notify,
    /// And a [`Verdict::Stop`] ends the worker, after a checkpoint.
    Stop,
}

/// The person's declaration: what the gate may do and what a task and a day may
/// cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    /// Dollars a task may cost, API-equivalent; `None` is no budget.
    pub task_usd: Option<f64>,
    /// Dollars a rolling day may cost, all workers together; `None` is none.
    pub day_usd: Option<f64>,
}

impl Settings {
    /// The settings out of a stored value, field by field: a field that is not
    /// what it should be falls back alone and does not take its neighbours
    /// with it, and a budget that is not a budget ([`BUDGET_USD_MAX`]) is none.
    #[must_use]
    pub fn parse(_value: &serde_json::Value) -> Self {
        Self::default()
    }
}

/// The numbers the verdict was read from, for the board to show beside it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Metrics {
    /// Tool calls this attempt made, as the window saw them.
    pub steps: u32,
    pub since_checkpoint: u32,
    pub checkpoints: u32,
    /// Thousandths of the latest calls that redid something or failed; `None`
    /// until the window holds [`REWORK_MIN_STEPS`] of them.
    pub rework_permille: Option<u32>,
    /// The latest model calls' cost against the ones before, in thousandths;
    /// `None` until twice [`RISE_WINDOW`] calls were read.
    pub rise_permille: Option<u32>,
    /// What the latest model calls cost, as a median, in dollars.
    pub step_usd: Option<f64>,
    /// What this attempt's model calls cost, in dollars, as far as they were
    /// read and priced; `None` when none was.
    pub spent_usd: Option<f64>,
    /// How many model calls had no price — what `spent_usd` is short of.
    pub unpriced_calls: u32,
}

/// The judgment: the verdict, every fact behind it (the most serious first)
/// and the numbers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Judgement {
    pub verdict: Verdict,
    pub reasons: Vec<Reason>,
    pub metrics: Metrics,
}

/// What a tool call is, for the question "is this the call it just made".
fn call_print(_payload: &str) -> Option<u64> {
    None
}

/// One attempt's record: what the window has seen it do.
#[derive(Clone, Debug, Default)]
pub struct StepBook {
    window: VecDeque<bool>,
    costs: VecDeque<f64>,
}

impl StepBook {
    /// One hook event of this attempt's pane.
    pub fn note_activity(&mut self, _activity: &Activity, _payload: &str) {}

    /// One model call's cost as its CLI's record gave it.
    pub fn note_cost(&mut self, _usd: Option<f64>) {}

    /// The window saved this attempt's state at `now_ms`.
    pub fn checkpointed(&mut self, _now_ms: i64) {}

    /// What this attempt's model calls cost so far.
    #[must_use]
    pub fn spent_usd(&self) -> Option<f64> {
        None
    }

    /// What this attempt is still to spend before a stop can land.
    #[must_use]
    pub fn ahead_usd(&self) -> f64 {
        0.0
    }

    /// The numbers the judgment reads, for a board to show.
    #[must_use]
    pub fn metrics(&self) -> Metrics {
        Metrics {
            steps: 0,
            since_checkpoint: 0,
            checkpoints: 0,
            rework_permille: None,
            rise_permille: None,
            step_usd: None,
            spent_usd: None,
            unpriced_calls: 0,
        }
    }

    /// The verdict, and every fact behind it, most serious first.
    #[must_use]
    pub fn judge(&self, _allowance: &Allowance, _now_ms: i64) -> Judgement {
        Judgement {
            verdict: Verdict::Continue,
            reasons: Vec::new(),
            metrics: self.metrics(),
        }
    }
}

#[cfg(test)]
mod tests;
