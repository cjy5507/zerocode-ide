//! The gate's memory in this window (t-26583).

#![allow(unused_imports, dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use zerocode_core::continue_gate::day::DaySpend;
use zerocode_core::continue_gate::plan::Standing;
use zerocode_core::continue_gate::spend::CallCost;
use zerocode_core::continue_gate::{Metrics, Mode, Reason, Settings, StepBook, Verdict};
use zerocode_core::hook::Activity;

use super::LedgerAgent;
use super::gate_meter::{CostNote, Meter};

/// How many panes the book remembers.
pub(super) const PANES_MAX: usize = 256;

/// How many tasks' spend the book remembers.
pub(super) const TASKS_MAX: usize = 512;

/// Whether the gate is on at all.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// What the person set, as the beat last read it.
static SETTINGS: Mutex<Settings> = Mutex::new(Settings {
    mode: Mode::Notify,
    task_usd: None,
    day_usd: None,
});

/// What the gate saved of a worker's tree, or why it could not.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct SnapshotNote {
    pub(crate) at_ms: i64,
    pub(crate) reference: Option<String>,
    pub(crate) error: Option<String>,
}

/// What the board shows of one worker's gate.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct GateReading {
    pub(crate) mode: Mode,
    pub(crate) verdict: Verdict,
    pub(crate) reasons: Vec<Reason>,
    pub(crate) metrics: Metrics,
    pub(crate) cost: CostNote,
    pub(crate) task_spent_usd: f64,
    pub(crate) task_limit_usd: Option<f64>,
    pub(crate) day_spent_usd: f64,
    pub(crate) day_limit_usd: Option<f64>,
    pub(crate) snapshot: Option<SnapshotNote>,
    pub(crate) at_ms: i64,
}

/// One pane's record.
pub(super) struct PaneGate {
    token: String,
    pub(super) book: StepBook,
    pub(super) standing: Standing,
    pub(super) meter: Option<Meter>,
    pub(super) cost: CostNote,
    pub(super) reading: Option<GateReading>,
    pub(super) snapshot: Option<SnapshotNote>,
    noted_ms: i64,
}

impl PaneGate {
    fn new(token: &str, now_ms: i64) -> Self {
        Self {
            token: token.to_string(),
            book: StepBook::default(),
            standing: Standing::default(),
            meter: None,
            cost: CostNote::default(),
            reading: None,
            snapshot: None,
            noted_ms: now_ms,
        }
    }

    /// The counts belong to `dispatch`.
    pub(super) fn for_attempt(&mut self, _dispatch: &str) {}
}

/// The window's one gate book.
#[derive(Default)]
pub(crate) struct GateBook {
    panes: HashMap<u32, PaneGate>,
    tasks: HashMap<String, f64>,
    day: DaySpend,
}

static BOOK: LazyLock<Mutex<GateBook>> = LazyLock::new(Mutex::default);

/// The window's one book.
pub(super) fn book() -> MutexGuard<'static, GateBook> {
    BOOK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a person set, as last read.
pub(super) fn settings() -> Settings {
    *SETTINGS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The settings the person just saved, or the beat just read.
pub(crate) fn set_settings(settings: Settings) {
    *SETTINGS.lock().unwrap_or_else(PoisonError::into_inner) = settings;
    ENABLED.store(settings.mode != Mode::Off, Ordering::Relaxed);
}

/// What the hook server says an agent just did, into the pane's record.
pub(crate) fn note_hook(
    _term: u32,
    _token: &str,
    _activity: &Activity,
    _payload: &str,
    _now_ms: i64,
) {
}

/// The board's rows, each with what the gate last said of its worker.
pub(crate) fn dress(agents: Vec<LedgerAgent>) -> Vec<LedgerAgent> {
    agents
}

impl GateBook {
    /// The record of `term`.
    pub(super) fn pane(&mut self, term: u32, token: &str, now_ms: i64) -> &mut PaneGate {
        let gate = self
            .panes
            .entry(term)
            .or_insert_with(|| PaneGate::new(token, now_ms));
        *gate = PaneGate::new(token, now_ms);
        gate
    }

    /// The record of `term`, when there is one.
    pub(super) fn pane_mut(&mut self, term: u32) -> Option<&mut PaneGate> {
        self.panes.get_mut(&term)
    }

    /// What the model calls a worker's CLI recorded cost.
    pub(super) fn add_costs(&mut self, _term: u32, _task: &str, _costs: &[CallCost], _now: i64) {}

    /// What a task has cost so far.
    pub(super) fn task_spent(&self, _task: &str) -> f64 {
        0.0
    }

    /// What the rolling day has cost so far.
    pub(super) fn day_spent(&self, _now_ms: i64) -> f64 {
        0.0
    }

    /// What the workers in `terms` are still to spend.
    pub(super) fn ahead_of(&self, _terms: &[u32]) -> f64 {
        0.0
    }

    /// Every worker's reading goes: the gate was turned off.
    pub(super) fn clear_readings(&mut self) {
        for pane in self.panes.values_mut() {
            pane.reading = None;
        }
    }

    /// The day's spend, when it changed since it was last taken for saving.
    pub(crate) fn take_day_for_saving(&mut self) -> Option<DaySpend> {
        None
    }

    /// The day as it was saved, read back at startup.
    pub(crate) fn restore_day(&mut self, _day: DaySpend) {}
}

#[cfg(test)]
mod tests;
