//! The continue gate on the beat (t-26583).

#![allow(unused_imports, dead_code)]

use std::path::Path;

use zerocode_core::continue_gate::plan::Act;
use zerocode_core::continue_gate::{Acted, Allowance, Cap, Judgement, Mode, Settings, Verdict};
use zerocode_core::orchestration::GateReceipt;

use super::gate_book::{self, PaneGate, SnapshotNote};
use super::gate_meter::CostNote;
use super::step_effort::WorkerPane;
use crate::agent_teams::Host;

/// What the gate does to the world, behind one door so a test can stand in for it.
pub(super) trait GateDoor {
    /// Saves `pane`'s tree as checkpoint `number`.
    fn snapshot(&self, pane: &WorkerPane, number: u32) -> Result<Option<String>, String>;

    /// Writes the receipt in the ledger's own voice.
    fn tell(&self, receipt: GateReceipt, now_ms: i64) -> bool;

    /// Ends the worker and puts a decision gate in front of its task.
    fn stop(&self, pane: &WorkerPane, why: &str, now_ms: i64) -> Result<(), String>;
}

/// What one beat knows about the spend around a worker.
pub(super) struct Around {
    pub(super) allowance: Allowance,
    pub(super) task_spent: f64,
    pub(super) day_spent: f64,
    pub(super) cost: CostNote,
}

/// The ledger's note of why a worker was ended: the budget it would have spent.
fn stop_reason(_judgement: &Judgement) -> String {
    "gate".to_string()
}

/// One worker's turn at the gate, whole — what the tests drive.
#[cfg(test)]
pub(super) fn step(
    _gate: &mut PaneGate,
    _pane: &WorkerPane,
    _settings: Settings,
    _around: &Around,
    _door: &dyn GateDoor,
    _now_ms: i64,
) {
}

/// The budgets that apply to one worker.
fn allowance_of(
    _settings: Settings,
    _task_spent: f64,
    _day_spent: f64,
    _own_ahead: f64,
    _day_ahead: f64,
) -> Allowance {
    Allowance::default()
}

/// One beat of the gate: every live worker read, judged, and acted on.
pub(super) fn sweep(
    _host: &dyn Host,
    _overrides: &[(String, zerocode_core::LaunchOverride)],
    _now_ms: i64,
) {
}

#[cfg(test)]
mod tests;
