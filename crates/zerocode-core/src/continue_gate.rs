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

pub mod day;
pub mod plan;
pub mod spend;

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::hook::{Activity, INPUT_KEYS, Phase, tool_name_in_parsed};
use crate::payload::HookPayload;

/// Tool calls between two forced checkpoints. Two hundred: the activity ring's
/// own comment counts an agent at work in "thousands" of calls an hour, so a
/// checkpoint comes every few minutes of real work — often enough that a lost
/// pane costs minutes, seldom enough that the snapshot (a status and a tree
/// write) is noise. A first estimate, calibrated against the checkpoint counts
/// the board shows after a week of use.
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
    /// Every verdict, lowest first — what a surface that must have a word for each
    /// walks.
    pub const ALL: [Self; 4] = [Self::Continue, Self::Checkpoint, Self::Pause, Self::Stop];

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

/// What the window did about a verdict it told the coordinator, for the receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Acted {
    /// It told, and ended nobody: the gate is set to tell, or the verdict is
    /// not one that ends work.
    Told,
    /// It saved the worker's tree and ended the worker.
    Stopped,
    /// It tried to end the worker and could not.
    StopFailed,
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
    /// Every code — what a surface that must have a sentence for each walks.
    pub const ALL: [Self; 7] = [
        Self::CheckpointDue,
        Self::CostRising,
        Self::ReworkLoop,
        Self::TaskBudgetNear,
        Self::DayBudgetNear,
        Self::TaskBudgetStop,
        Self::DayBudgetStop,
    ];

    /// The verdict this fact alone earns.
    #[must_use]
    pub const fn verdict(self) -> Verdict {
        match self {
            Self::CheckpointDue | Self::CostRising => Verdict::Checkpoint,
            Self::ReworkLoop | Self::TaskBudgetNear | Self::DayBudgetNear => Verdict::Pause,
            Self::TaskBudgetStop | Self::DayBudgetStop => Verdict::Stop,
        }
    }

    /// The word this code is serialized as — what a row and a screen key on.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::CheckpointDue => "checkpoint_due",
            Self::CostRising => "cost_rising",
            Self::ReworkLoop => "rework_loop",
            Self::TaskBudgetNear => "task_budget_near",
            Self::DayBudgetNear => "day_budget_near",
            Self::TaskBudgetStop => "task_budget_stop",
            Self::DayBudgetStop => "day_budget_stop",
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Settings {
    pub mode: Mode,
    /// Dollars a task may cost, API-equivalent; `None` is no budget.
    pub task_usd: Option<f64>,
    /// Dollars a rolling day may cost, all workers together; `None` is none.
    pub day_usd: Option<f64>,
}

/// Whether a figure can be a budget: a positive, finite number under the typo
/// guard ([`BUDGET_USD_MAX`]).
fn is_budget(usd: &f64) -> bool {
    usd.is_finite() && *usd > 0.0 && *usd <= BUDGET_USD_MAX
}

impl<'de> Deserialize<'de> for Settings {
    /// Read from the settings file the way [`Settings::parse`] reads what the
    /// settings pane sends: field by field, a mess is the default and never an
    /// error — the settings document is one file, and a file that does not parse is
    /// set aside with every other setting in it.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&serde_json::Value::deserialize(deserializer)?))
    }
}

impl Settings {
    /// The settings out of a stored value, field by field: a field that is not
    /// what it should be falls back alone and does not take its neighbours
    /// with it, and a budget that is not a budget ([`BUDGET_USD_MAX`]) is none.
    #[must_use]
    pub fn parse(value: &serde_json::Value) -> Self {
        let usd = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .filter(is_budget)
        };
        Self {
            mode: value
                .get("mode")
                .and_then(|mode| serde_json::from_value(mode.clone()).ok())
                .unwrap_or_default(),
            task_usd: usd("task_usd"),
            day_usd: usd("day_usd"),
        }
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

/// The median of some numbers, or `None` for none.
fn median(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut sorted: Vec<f64> = values.collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    })
}

/// `part` of `whole`, in thousandths.
fn permille(part: u32, whole: usize) -> u32 {
    let whole = u64::try_from(whole).unwrap_or(u64::MAX).max(1);
    u32::try_from(u64::from(part) * 1_000 / whole).unwrap_or(u32::MAX)
}

/// `top` over `bottom`, in thousandths, saturating.
fn ratio_permille(top: f64, bottom: f64) -> u32 {
    (top / bottom * 1_000.0)
        .round()
        .clamp(0.0, f64::from(u32::MAX)) as u32
}

/// What a tool call is, for the question "is this the call it just made": a
/// hash of the tool's name and the first [`PRINT_BYTES`] bytes of its input's
/// words. `None` for an event that names no tool — it cannot repeat anything.
fn call_print(payload: &str) -> Option<u64> {
    let payload = HookPayload::of(payload);
    let name = tool_name_in_parsed(&payload)?;
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    if let Some(input) = payload
        .tree()
        .and_then(|tree| INPUT_KEYS.iter().find_map(|key| tree.get(*key)))
    {
        let mut room = PRINT_BYTES;
        feed(input, &mut hasher, &mut room);
    }
    Some(hasher.finish())
}

/// The words of one input value into a hash, spending at most `room` bytes.
fn feed(value: &serde_json::Value, hasher: &mut DefaultHasher, room: &mut usize) {
    use serde_json::Value;
    if *room == 0 {
        return;
    }
    match value {
        Value::String(text) => {
            let taken = text.len().min(*room);
            hasher.write(&text.as_bytes()[..taken]);
            *room -= taken;
        }
        Value::Array(items) => {
            for item in items {
                feed(item, hasher, room);
            }
        }
        Value::Object(fields) => {
            for (key, item) in fields {
                hasher.write(key.as_bytes());
                feed(item, hasher, room);
            }
        }
        other => {
            let text = other.to_string();
            let taken = text.len().min(*room);
            hasher.write(&text.as_bytes()[..taken]);
            *room -= taken;
        }
    }
}

/// One attempt's record: what the window has seen it do, as little of it as the
/// judgment reads. Small and bounded — a window of marks, a ring of costs, a few
/// counters — so a window holding a hundred workers holds a few kilobytes of
/// this.
#[derive(Clone, Debug, Default)]
pub struct StepBook {
    steps: u32,
    since_checkpoint: u32,
    checkpoints: u32,
    last_checkpoint_ms: Option<i64>,
    /// The print of the call just made, while a repeat of it would be rework.
    last_call: Option<u64>,
    /// The latest calls, newest last: whether each redid something or failed.
    window: VecDeque<bool>,
    rework_in_window: u32,
    /// What each of the latest model calls cost, in dollars, newest last.
    costs: VecDeque<f64>,
    spent_usd: f64,
    priced_calls: u32,
    unpriced_calls: u32,
}

impl StepBook {
    /// One hook event of this attempt's pane. A tool call starting is a step,
    /// and a repeat of the call before it is rework; a call that failed is
    /// rework; a prompt is a new instruction, which no run of repeats crosses.
    /// Everything else — a call finishing, a turn ending — says nothing the
    /// gate reads.
    pub fn note_activity(&mut self, activity: &Activity, payload: &str) {
        match activity.phase {
            Phase::Started => self.note_call(payload),
            Phase::Failed => self.note_failure(),
            Phase::Prompted => self.last_call = None,
            Phase::Finished | Phase::Stopped => {}
        }
    }

    fn note_call(&mut self, payload: &str) {
        self.steps = self.steps.saturating_add(1);
        self.since_checkpoint = self.since_checkpoint.saturating_add(1);
        let print = call_print(payload);
        let repeat = print.is_some() && print == self.last_call;
        self.last_call = print;
        self.window.push_back(repeat);
        if repeat {
            self.rework_in_window += 1;
        }
        while self.window.len() > REWORK_WINDOW_STEPS {
            if self.window.pop_front() == Some(true) {
                self.rework_in_window -= 1;
            }
        }
    }

    /// The call that just failed was rework, whether or not it repeated one.
    fn note_failure(&mut self) {
        if let Some(newest) = self.window.back_mut()
            && !*newest
        {
            *newest = true;
            self.rework_in_window += 1;
        }
    }

    /// One model call's cost as its CLI's record gave it: dollars, or `None` for
    /// a model with no price — which counts against nothing and says so.
    pub fn note_cost(&mut self, usd: Option<f64>) {
        match usd.filter(|usd| usd.is_finite() && *usd >= 0.0) {
            Some(usd) => {
                self.spent_usd += usd;
                self.priced_calls = self.priced_calls.saturating_add(1);
                self.remember(usd);
            }
            None => self.unpriced_calls = self.unpriced_calls.saturating_add(1),
        }
    }

    /// One model call the window only LEARNED OF: it finished before the window
    /// began to watch, so what it cost is nobody's spend here — the day's total
    /// already holds it if a window watched it, and a window that did not has no
    /// claim on it — but it says how dear this worker's calls are, which the trend
    /// and the projection of a stop need from the first beat. A cost that is not a
    /// number of dollars is nothing.
    pub fn seed_cost(&mut self, usd: f64) {
        if usd.is_finite() && usd >= 0.0 {
            self.remember(usd);
        }
    }

    /// The latest calls' costs, newest last, and no more than the ring holds.
    fn remember(&mut self, usd: f64) {
        self.costs.push_back(usd);
        while self.costs.len() > COST_RING {
            self.costs.pop_front();
        }
    }

    /// The window saved this attempt's state at `now_ms`.
    pub fn checkpointed(&mut self, now_ms: i64) {
        self.since_checkpoint = 0;
        self.checkpoints = self.checkpoints.saturating_add(1);
        self.last_checkpoint_ms = Some(now_ms);
    }

    /// What this attempt's model calls cost so far, in dollars, as far as they
    /// were read and priced; `None` when none was.
    #[must_use]
    pub fn spent_usd(&self) -> Option<f64> {
        (self.priced_calls > 0).then_some(self.spent_usd)
    }

    /// What this attempt is still to spend before a stop can land: the next
    /// [`PROJECTION_STEPS`] calls at the median of the latest — nothing until
    /// [`PROJECTION_MIN_CALLS`] were read.
    #[must_use]
    pub fn ahead_usd(&self) -> f64 {
        self.recent_step_usd()
            .map_or(0.0, |step| step * f64::from(PROJECTION_STEPS))
    }

    fn recent_step_usd(&self) -> Option<f64> {
        if self.costs.len() < PROJECTION_MIN_CALLS {
            return None;
        }
        median(
            self.costs
                .iter()
                .rev()
                .take(PROJECTION_RECENT_CALLS)
                .copied(),
        )
    }

    fn rework_permille(&self) -> Option<u32> {
        (self.window.len() >= REWORK_MIN_STEPS)
            .then(|| permille(self.rework_in_window, self.window.len()))
    }

    /// The latest calls against the ones before, as a pair: the ratio in
    /// thousandths and the latest median.
    fn rise(&self) -> Option<(u32, f64)> {
        if self.costs.len() < 2 * RISE_WINDOW {
            return None;
        }
        let latest = median(self.costs.iter().rev().take(RISE_WINDOW).copied())?;
        let before = median(
            self.costs
                .iter()
                .rev()
                .skip(RISE_WINDOW)
                .take(RISE_WINDOW)
                .copied(),
        )?;
        (before > 0.0).then(|| (ratio_permille(latest, before), latest))
    }

    /// The numbers the judgment reads, for a board to show.
    #[must_use]
    pub fn metrics(&self) -> Metrics {
        Metrics {
            steps: self.steps,
            since_checkpoint: self.since_checkpoint,
            checkpoints: self.checkpoints,
            rework_permille: self.rework_permille(),
            rise_permille: self.rise().map(|(ratio, _)| ratio),
            step_usd: self.recent_step_usd(),
            spent_usd: self.spent_usd(),
            unpriced_calls: self.unpriced_calls,
        }
    }

    /// The verdict, and every fact behind it, most serious first.
    #[must_use]
    pub fn judge(&self, allowance: &Allowance, now_ms: i64) -> Judgement {
        let mut reasons = Vec::new();
        for (stop, near, cap) in [
            (Code::TaskBudgetStop, Code::TaskBudgetNear, allowance.task),
            (Code::DayBudgetStop, Code::DayBudgetNear, allowance.day),
        ] {
            if let Some(cap) = cap.filter(|cap| cap.limit_usd > 0.0) {
                if cap.spent_usd + cap.ahead_usd >= cap.limit_usd {
                    reasons.push(Reason {
                        code: stop,
                        value: cap.spent_usd + cap.ahead_usd,
                        limit: cap.limit_usd,
                    });
                } else if cap.spent_usd * 1_000.0 >= cap.limit_usd * f64::from(BUDGET_NEAR_PERMILLE)
                {
                    reasons.push(Reason {
                        code: near,
                        value: cap.spent_usd,
                        limit: cap.limit_usd,
                    });
                }
            }
        }
        if let Some(rework) = self
            .rework_permille()
            .filter(|rework| *rework > REWORK_LIMIT_PERMILLE)
        {
            reasons.push(Reason {
                code: Code::ReworkLoop,
                value: f64::from(rework),
                limit: f64::from(REWORK_LIMIT_PERMILLE),
            });
        }
        if let Some((ratio, latest)) = self.rise()
            && ratio >= RISE_FACTOR_PERMILLE
            && latest >= RISE_MIN_USD
        {
            reasons.push(Reason {
                code: Code::CostRising,
                value: f64::from(ratio),
                limit: f64::from(RISE_FACTOR_PERMILLE),
            });
        }
        let waited = self
            .last_checkpoint_ms
            .is_none_or(|at_ms| now_ms.saturating_sub(at_ms) >= CHECKPOINT_MIN_GAP_MS);
        if self.since_checkpoint >= CHECKPOINT_EVERY_STEPS && waited {
            reasons.push(Reason {
                code: Code::CheckpointDue,
                value: f64::from(self.since_checkpoint),
                limit: f64::from(CHECKPOINT_EVERY_STEPS),
            });
        }
        reasons.sort_by_key(|reason| std::cmp::Reverse(reason.code.verdict()));
        Judgement {
            verdict: reasons
                .first()
                .map_or(Verdict::Continue, |reason| reason.code.verdict()),
            reasons,
            metrics: self.metrics(),
        }
    }
}

#[cfg(test)]
mod tests;
