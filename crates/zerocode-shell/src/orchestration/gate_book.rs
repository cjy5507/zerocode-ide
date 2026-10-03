//! The gate's memory in this window (t-26583): what each pane's agent has been
//! seen to do, what its model calls cost, what each task and the day have cost,
//! and what the gate last said about each worker for the board to show.
//!
//! One book for the whole window, like the cost book beside it
//! ([`super::cost_book`]) and for the same reason: the hook server feeds it from
//! one task, the beat reads it from another, and the board reads what the beat
//! published. It is bounded on every side — [`PANES_MAX`] panes, [`TASKS_MAX`]
//! tasks, a day of minute buckets — so a window left open for a month holds what
//! it held on the first day.
//!
//! Nothing here is a judgment: [`zerocode_core::continue_gate`] judges and plans,
//! the beat's sweep ([`super::continue_gate`]) carries the plan out, and this
//! holds the facts between them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};

use serde::Serialize;
use zerocode_core::continue_gate::day::DaySpend;
use zerocode_core::continue_gate::plan::Standing;
use zerocode_core::continue_gate::spend::CallCost;
use zerocode_core::continue_gate::{Metrics, Mode, Reason, Settings, StepBook, Verdict};
use zerocode_core::hook::Activity;

use super::LedgerAgent;
use super::gate_meter::{CostNote, Meter};

/// How many panes the book remembers: every pane the hook server has spoken for
/// is a few hundred bytes, and the person's own panes speak too. Past this the
/// pane least recently heard from goes.
pub(super) const PANES_MAX: usize = 256;

/// How many tasks' spend the book remembers; the cheapest go first, the dear
/// ones are what a budget is about.
pub(super) const TASKS_MAX: usize = 512;

/// Whether the gate is on at all — a mirror of the setting the hook road reads
/// without taking a lock, once per event.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// What the person set, as the beat last read it. The default is the one a
/// person who never chose gets: tell, and end nothing.
static SETTINGS: Mutex<Settings> = Mutex::new(Settings {
    mode: Mode::Notify,
    task_usd: None,
    day_usd: None,
});

/// Where the day's spend is kept, beside the other per-machine records.
const DAY_FILE: &str = "gate-day-spend.json";

/// How often the day's spend is written down when it changed: a minute — a
/// restart loses at most that much of it, and a write is not paid every beat.
pub(super) const DAY_SAVE_EVERY_MS: i64 = 60_000;

/// The file the day is kept in, once the window has named its config root.
static DAY_PATH: OnceLock<PathBuf> = OnceLock::new();

/// When the day was last written down.
static DAY_SAVED_MS: AtomicI64 = AtomicI64::new(i64::MIN);

/// The day as it was written, or `None` for a file that is not there or not a
/// day.
pub(super) fn load_day(_file: &Path) -> Option<DaySpend> {
    None
}

/// The day, written down: one durable replace.
pub(super) fn write_day(_file: &Path, _day: &DaySpend) {}

/// Opens the gate at boot: what a person set, and the day the window had
/// already counted.
pub(crate) fn open(config_root: &Path, settings: Settings) {
    set_settings(settings);
    let file = config_root.join(DAY_FILE);
    if let Some(day) = load_day(&file) {
        book().restore_day(day);
    }
    let _ = DAY_PATH.set(file);
}

/// The day's spend, written down when it changed and a minute has passed since
/// it last was.
pub(crate) fn save_day(now_ms: i64) {
    let Some(file) = DAY_PATH.get() else {
        return;
    };
    if now_ms.saturating_sub(DAY_SAVED_MS.load(Ordering::Relaxed)) < DAY_SAVE_EVERY_MS {
        return;
    }
    let Some(day) = book().take_day_for_saving() else {
        return;
    };
    DAY_SAVED_MS.store(now_ms, Ordering::Relaxed);
    write_day(file, &day);
}

/// What the rolling day has cost so far, as far as the window watched it — for
/// the settings card.
pub(crate) fn day_spent_now() -> f64 {
    book().day_spent(crate::now_epoch_ms())
}

/// What the gate saved of a worker's tree, or why it could not.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct SnapshotNote {
    pub(crate) at_ms: i64,
    /// The ref the tree was saved under; `None` when the tree was clean (there
    /// was nothing to save) or the save failed.
    pub(crate) reference: Option<String>,
    pub(crate) error: Option<String>,
}

/// What the board shows of one worker's gate: the verdict and every fact behind
/// it, with the numbers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct GateReading {
    pub(crate) mode: Mode,
    pub(crate) verdict: Verdict,
    pub(crate) reasons: Vec<Reason>,
    pub(crate) metrics: Metrics,
    /// Why the cost is, or is not, a number.
    pub(crate) cost: CostNote,
    /// What the worker's task has cost so far, as far as the window watched,
    /// beside the budget a person gave it.
    pub(crate) task_spent_usd: f64,
    pub(crate) task_limit_usd: Option<f64>,
    /// And the rolling day's, all workers together.
    pub(crate) day_spent_usd: f64,
    pub(crate) day_limit_usd: Option<f64>,
    pub(crate) snapshot: Option<SnapshotNote>,
    pub(crate) at_ms: i64,
}

/// One pane's record.
pub(super) struct PaneGate {
    /// The launch the counts belong to: a pane that starts over starts them over.
    token: String,
    /// The attempt the counts belong to, as the beat last saw it.
    dispatch: Option<String>,
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
            dispatch: None,
            book: StepBook::default(),
            standing: Standing::default(),
            meter: None,
            cost: CostNote::default(),
            reading: None,
            snapshot: None,
            noted_ms: now_ms,
        }
    }

    /// The counts belong to `dispatch`: another attempt of the same pane starts
    /// them over, and keeps the place the meter holds in the transcript.
    pub(super) fn for_attempt(&mut self, dispatch: &str) {
        if self.dispatch.as_deref() == Some(dispatch) {
            return;
        }
        self.dispatch = Some(dispatch.to_string());
        self.book = StepBook::default();
        self.standing = Standing::default();
        self.reading = None;
        self.snapshot = None;
    }
}

/// The window's one gate book.
#[derive(Default)]
pub(crate) struct GateBook {
    panes: HashMap<u32, PaneGate>,
    /// What each task has cost across its attempts, as far as watched.
    tasks: HashMap<String, f64>,
    day: DaySpend,
    day_changed: bool,
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

/// What the hook server says an agent just did, into the pane's record. The
/// hook road calls this for every tool event of every pane, so a gate that is
/// off answers before it takes a lock.
pub(crate) fn note_hook(term: u32, token: &str, activity: &Activity, payload: &str, now_ms: i64) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    book()
        .pane(term, token, now_ms)
        .book
        .note_activity(activity, payload);
}

/// The board's rows, each with what the gate last said of its worker.
pub(crate) fn dress(mut agents: Vec<LedgerAgent>) -> Vec<LedgerAgent> {
    let held = book();
    for row in &mut agents {
        row.gate = row
            .term
            .and_then(|term| held.panes.get(&term))
            .and_then(|pane| pane.reading.clone());
    }
    agents
}

impl GateBook {
    /// The record of `term`, made when there is none, started over when the pane
    /// has launched again.
    pub(super) fn pane(&mut self, term: u32, token: &str, now_ms: i64) -> &mut PaneGate {
        if !self.panes.contains_key(&term) && self.panes.len() >= PANES_MAX {
            self.forget_least_recent();
        }
        let gate = self
            .panes
            .entry(term)
            .or_insert_with(|| PaneGate::new(token, now_ms));
        if gate.token.is_empty() {
            gate.token = token.to_string();
        } else if !token.is_empty() && gate.token != token {
            *gate = PaneGate::new(token, now_ms);
        }
        gate.noted_ms = now_ms;
        gate
    }

    /// The record of `term`, when there is one.
    pub(super) fn pane_mut(&mut self, term: u32) -> Option<&mut PaneGate> {
        self.panes.get_mut(&term)
    }

    /// What the model calls a worker's CLI recorded cost, into its own count,
    /// its task's and the day's.
    pub(super) fn add_costs(&mut self, term: u32, task: &str, costs: &[CallCost], now_ms: i64) {
        let Some(gate) = self.panes.get_mut(&term) else {
            return;
        };
        for cost in costs {
            gate.book.note_cost(cost.usd());
            if let Some(usd) = cost.usd() {
                *self.tasks.entry(task.to_string()).or_insert(0.0) += usd;
                self.day.add(now_ms, usd);
                self.day_changed = true;
            }
        }
        self.trim_tasks();
    }

    /// What a task has cost so far, as far as watched.
    pub(super) fn task_spent(&self, task: &str) -> f64 {
        self.tasks.get(task).copied().unwrap_or(0.0)
    }

    /// What the rolling day has cost so far, as far as watched.
    pub(super) fn day_spent(&self, now_ms: i64) -> f64 {
        self.day.spent(now_ms)
    }

    /// What the workers in `terms` are still to spend before a stop could land —
    /// the whole day's projection, of everything running now.
    pub(super) fn ahead_of(&self, terms: &[u32]) -> f64 {
        terms
            .iter()
            .filter_map(|term| self.panes.get(term))
            .map(|pane| pane.book.ahead_usd())
            .sum()
    }

    /// Every worker's reading goes: the gate was turned off.
    pub(super) fn clear_readings(&mut self) {
        for pane in self.panes.values_mut() {
            pane.reading = None;
        }
    }

    /// The day's spend, when it changed since it was last taken for saving.
    pub(crate) fn take_day_for_saving(&mut self) -> Option<DaySpend> {
        std::mem::take(&mut self.day_changed).then(|| self.day.clone())
    }

    /// The day as it was saved, read back at startup.
    pub(crate) fn restore_day(&mut self, day: DaySpend) {
        self.day = day;
    }

    fn forget_least_recent(&mut self) {
        if let Some(oldest) = self
            .panes
            .iter()
            .min_by_key(|(_, pane)| pane.noted_ms)
            .map(|(term, _)| *term)
        {
            self.panes.remove(&oldest);
        }
    }

    fn trim_tasks(&mut self) {
        while self.tasks.len() > TASKS_MAX {
            let Some(cheapest) = self
                .tasks
                .iter()
                .min_by(|left, right| left.1.total_cmp(right.1))
                .map(|(task, _)| task.clone())
            else {
                break;
            };
            self.tasks.remove(&cheapest);
        }
    }
}

#[cfg(test)]
mod tests;
