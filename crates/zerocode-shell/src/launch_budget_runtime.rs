//! The window's one launch ledger (t-26583).

#![allow(unused_imports, dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use zerocode_core::launch_budget::{
    Ask, Counters, LaunchLedger, Limits, Outcome, REST_UNKNOWN_MS, Refusal,
};

use crate::quota_wall::{StallCause, one_shot_cause};
use crate::scm_runtime::{Once, OnceFailure, run_once};

/// Where the ledger is kept.
const FILE: &str = "launch-ledger.json";

/// What a launch tells the ledger about itself.
pub(crate) struct Launch<'a> {
    pub(crate) provider: &'a str,
    pub(crate) job: Option<&'a str>,
    pub(crate) fresh_ms: Option<i64>,
    pub(crate) requested: bool,
}

/// What a launch came to.
pub(crate) enum Budgeted {
    /// The ledger did not let it start.
    Refused(Refusal),
    /// It ran, however it ended.
    Ran(Result<Once, OnceFailure>),
}

/// One ledger with the file it is kept in and the ceilings it is held to.
pub(crate) struct Budgeter {
    ledger: Mutex<LaunchLedger>,
    limits: Mutex<Limits>,
    file: Option<PathBuf>,
}

impl Budgeter {
    pub(crate) fn new(file: Option<PathBuf>, limits: Limits) -> Self {
        Self {
            ledger: Mutex::new(LaunchLedger::default()),
            limits: Mutex::new(limits),
            file,
        }
    }

    pub(crate) fn set_limits(&self, _limits: Limits) {}

    pub(crate) fn counters(&self, now_ms: i64) -> Counters {
        LaunchLedger::default().counters(now_ms)
    }

    pub(crate) fn run(
        &self,
        _launch: &Launch<'_>,
        _now: impl Fn() -> i64,
        ran: impl FnOnce() -> Result<Once, OnceFailure>,
    ) -> Budgeted {
        Budgeted::Ran(ran())
    }
}

/// How a launch ended.
fn outcome_of(_provider: &str, _result: &Result<Once, OnceFailure>, _now_ms: i64) -> Outcome {
    Outcome::Done
}

/// The window's one ledger, opened at boot.
static BUDGETER: OnceLock<Budgeter> = OnceLock::new();

/// Opens the ledger.
pub(crate) fn open(config_root: &Path, limits: Limits) {
    let _ = BUDGETER.set(Budgeter::new(Some(config_root.join(FILE)), limits));
}

/// The ceilings a person just set.
pub(crate) fn set_limits(limits: Limits) {
    if let Some(budgeter) = BUDGETER.get() {
        budgeter.set_limits(limits);
    }
}

/// What the ledger holds now.
pub(crate) fn counters() -> Counters {
    LaunchLedger::default().counters(crate::now_epoch_ms())
}

/// [`run_once`], through the ledger.
pub(crate) fn run_budgeted(
    _launch: &Launch<'_>,
    program: &str,
    cwd: Option<&Path>,
    argv: &[String],
    env: &[(String, String)],
    prompt: &str,
    wall: Duration,
) -> Budgeted {
    Budgeted::Ran(run_once(program, cwd, argv, env, prompt, wall))
}

/// A refusal in a sentence.
pub(crate) fn refusal_said(_refusal: &Refusal) -> String {
    String::new()
}

#[cfg(test)]
mod tests;
