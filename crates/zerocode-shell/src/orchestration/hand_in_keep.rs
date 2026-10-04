//! Keeping what a worker's hand-in names (t-32798) — the window's half.
//!
//! Today a `worker_done` that names files keeps nothing of them but one report,
//! copied verbatim, and no road that takes a checkout asks whether anything is
//! owed. This is the shape the keeping asks of the window — a job, a roster of
//! roots, a clearance a cleanup is told, a mode of asking — with a keeping that
//! keeps nothing and a clearance that is always clear. The tests in `tests.rs`
//! fail on that.

use std::path::{Path, PathBuf};

use zerocode_core::artifact::Origin;
use zerocode_core::hand_in::{HandIn, Manifest};

use super::LedgerAgent;
use super::desk::DeskSnapshot;
use crate::artifact_runtime::Store;

/// Where the keeping may read from.
#[derive(Debug, Clone, Default)]
pub(crate) struct Roots {
    checkout: Option<PathBuf>,
    temps: Vec<PathBuf>,
}

impl Roots {
    /// No root is known: neither the worker's checkout nor the temporary folders.
    pub(crate) fn of(_checkout: Option<&str>) -> Self {
        Self::default()
    }

    /// No path is held by roots that are not known.
    fn holds(&self, _path: &Path) -> bool {
        false
    }
}

/// One hand-in to keep.
#[derive(Debug, Clone)]
pub(crate) struct Job {
    pub(crate) hand_in: HandIn,
    pub(crate) origin: Origin,
    pub(crate) roots: Roots,
}

/// What a cleanup is told about the hand-ins a checkout holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Clearance {
    Clear,
    Keeping(String),
    Held(String),
}

/// Who is asking, and so what a keeping may cost them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Wait,
    Now,
}

/// Nothing is read and nothing is kept: a manifest with nothing in it.
pub(crate) fn ensure(_store: &Store, job: &Job, now_ms: i64) -> Manifest {
    Manifest::begin(&job.hand_in, String::new(), now_ms)
}

/// A cleanup is never made to wait, and no keeping is ever started.
pub(crate) fn clearance(
    _store: Option<&Store>,
    _jobs: &[Job],
    _mode: Mode,
    _now_ms: i64,
) -> (Clearance, Vec<Job>) {
    (Clearance::Clear, Vec::new())
}

/// Whether a ledger row's checkout is the one asked about: no row is.
fn is_the_checkout(_at: &str, _checkout: &Path) -> bool {
    false
}

/// The question every road that takes a checkout would ask: nothing is owed.
pub(crate) fn before_cleanup(_checkout: &Path, _mode: Mode) -> Clearance {
    Clearance::Clear
}

/// A hand-in's place among the keepings in flight: no table is kept, so every
/// one enters.
pub(crate) struct Flight;

impl Flight {
    fn enter(_job: &Job, _now_ms: i64) -> Option<Self> {
        Some(Self)
    }
}

/// The board's rows are dressed with nothing.
pub(crate) fn dress_desk_with(_store: &Store, _desk: &mut DeskSnapshot) {}

/// The roster is dressed with nothing.
pub(crate) fn dress_agents_with(_store: &Store, _agents: &mut [LedgerAgent]) {}

#[cfg(test)]
mod tests;
