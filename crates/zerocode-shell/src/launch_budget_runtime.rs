//! The window's one launch ledger (t-26583): every headless run of an agent's CLI
//! that the window starts for itself asks here first, and tells here how it ended.
//!
//! The rules are core's ([`zerocode_core::launch_budget`]); this is the clock, the
//! file and the one door ([`run_budgeted`]) over [`crate::scm_runtime::run_once`],
//! the plumbing every such run already shares — a commit draft, a Computer Use
//! value, a login renewal. A launch that is let through runs exactly as it always
//! did; one that is refused never starts a process, and says why.
//!
//! **A person's own button is counted and never held to the ceilings.** A
//! commit draft pressed while the hourly ceiling stands would be the window
//! refusing a person to protect them from themselves; only a provider that is
//! resting (it would have failed anyway) or the same job already running (a double
//! click) holds one back. The ceilings are for the launches nobody pressed.
//!
//! **A ledger that was never opened counts nothing and refuses nothing**: the
//! window opens it at boot, and every test that reaches `run_once` through a road
//! built on it keeps running the way it ran before.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use zerocode_core::launch_budget::{
    Ask, Counters, LaunchLedger, Limits, Outcome, REST_UNKNOWN_MS, Refusal,
};

use crate::quota_wall::{StallCause, one_shot_cause};
use crate::scm_runtime::{Once, OnceFailure, run_once};

/// Where the ledger is kept, beside the other per-machine records.
const FILE: &str = "launch-ledger.json";

/// What Claude Code's one JSON result carries when the run failed under a zero
/// exit: the failure is in the result, not in the status.
const ERROR_MARK: &str = "\"is_error\":true";

/// What a launch tells the ledger about itself.
pub(crate) struct Launch<'a> {
    /// Whose plan it spends — the agent's catalog id (`claude`, `codex`).
    pub(crate) provider: &'a str,
    /// The job, by [`zerocode_core::launch_budget::job_key`], when two launches of
    /// it are one job.
    pub(crate) job: Option<&'a str>,
    /// How long a finished run of the job stands as its answer, for a caller that
    /// can reuse one.
    pub(crate) fresh_ms: Option<i64>,
    /// A person asked for it by pressing a button: counted, never held to the
    /// ceilings.
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
    /// A ledger kept in `file`, read back from it when it is there.
    pub(crate) fn new(file: Option<PathBuf>, limits: Limits) -> Self {
        let ledger = file
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<LaunchLedger>(&text).ok())
            .unwrap_or_default();
        Self {
            ledger: Mutex::new(ledger),
            limits: Mutex::new(limits),
            file,
        }
    }

    /// The ceilings a person just set.
    pub(crate) fn set_limits(&self, limits: Limits) {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner) = limits;
    }

    /// What the ledger holds at `now_ms`.
    pub(crate) fn counters(&self, now_ms: i64) -> Counters {
        self.ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .counters(now_ms)
    }

    /// One launch: asked for a place, run, and told how it ended. The ledger's
    /// lock is never held while the launch runs, so another can ask — and be
    /// refused for being the same job — in the meantime.
    pub(crate) fn run(
        &self,
        launch: &Launch<'_>,
        now: impl Fn() -> i64,
        ran: impl FnOnce() -> Result<Once, OnceFailure>,
    ) -> Budgeted {
        let ceilings = if launch.requested {
            Limits {
                concurrent: None,
                per_hour: None,
                per_day: None,
            }
        } else {
            *self.limits.lock().unwrap_or_else(PoisonError::into_inner)
        };
        let permit = {
            let mut ledger = self.ledger.lock().unwrap_or_else(PoisonError::into_inner);
            let ask = Ask {
                provider: launch.provider,
                job: launch.job,
                fresh_ms: launch.fresh_ms,
            };
            match ledger.reserve(&ask, &ceilings, now()) {
                Ok(permit) => {
                    self.save(&ledger);
                    permit
                }
                Err(refusal) => return Budgeted::Refused(refusal),
            }
        };
        let result = ran();
        let ended_ms = now();
        let outcome = outcome_of(launch.provider, &result, ended_ms);
        let mut ledger = self.ledger.lock().unwrap_or_else(PoisonError::into_inner);
        ledger.finish(permit, outcome, ended_ms);
        self.save(&ledger);
        Budgeted::Ran(result)
    }

    /// The ledger, written down. A failed write is dropped, like a statistic's:
    /// the ledger stays right in memory until the next restart, and no launch
    /// should fail because its count could not be saved.
    fn save(&self, ledger: &LaunchLedger) {
        let Some(path) = &self.file else {
            return;
        };
        let Ok(text) = serde_json::to_string(ledger) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = crate::durable_file::ensure_private_directory(parent);
        }
        let _ = crate::durable_file::replace_bytes(path, text.as_bytes());
    }
}

/// How a launch ended, as far as the ledger needs: done, failed, or standing at
/// the provider's wall — read by the same words the window reads a pane's wall by
/// ([`one_shot_cause`]).
fn outcome_of(provider: &str, result: &Result<Once, OnceFailure>, now_ms: i64) -> Outcome {
    let Ok(once) = result else {
        return Outcome::Failed;
    };
    if once.success && !once.stdout.contains(ERROR_MARK) {
        return Outcome::Done;
    }
    let said = format!("{}\n{}", once.stderr_tail, once.stdout);
    if matches!(one_shot_cause(provider, &said), Some(StallCause::QuotaWall)) {
        Outcome::Rested {
            until_ms: now_ms.saturating_add(REST_UNKNOWN_MS),
        }
    } else {
        Outcome::Failed
    }
}

/// The window's one ledger, opened at boot.
static BUDGETER: OnceLock<Budgeter> = OnceLock::new();

/// Opens the ledger, kept under `config_root`, with the ceilings a person set.
pub(crate) fn open(config_root: &Path, limits: Limits) {
    let _ = BUDGETER.set(Budgeter::new(Some(config_root.join(FILE)), limits));
}

/// The ceilings a person just set.
pub(crate) fn set_limits(limits: Limits) {
    if let Some(budgeter) = BUDGETER.get() {
        budgeter.set_limits(limits);
    }
}

/// What the ledger holds now, for the settings card to say.
pub(crate) fn counters() -> Counters {
    let now_ms = crate::now_epoch_ms();
    BUDGETER.get().map_or_else(
        || LaunchLedger::default().counters(now_ms),
        |budgeter| budgeter.counters(now_ms),
    )
}

/// [`run_once`], through the ledger: the one door every headless run of an
/// agent's CLI goes in by.
pub(crate) fn run_budgeted(
    launch: &Launch<'_>,
    program: &str,
    cwd: Option<&Path>,
    argv: &[String],
    env: &[(String, String)],
    prompt: &str,
    wall: Duration,
) -> Budgeted {
    let ran = || run_once(program, cwd, argv, env, prompt, wall);
    match BUDGETER.get() {
        Some(budgeter) => budgeter.run(launch, crate::now_epoch_ms, ran),
        None => Budgeted::Ran(ran()),
    }
}

/// A refusal in a sentence — what a person reads where the draft would have been.
pub(crate) fn refusal_said(refusal: &Refusal) -> String {
    let now_ms = crate::now_epoch_ms();
    let minutes = |ms: i64| zerocode_core::orchestration::minutes_up(ms);
    match refusal {
        Refusal::Resting { until_ms } => format!(
            "한도 벽에 막혀 쉬는 중입니다 — 약 {}분 뒤에 다시 시도합니다",
            minutes(until_ms.saturating_sub(now_ms))
        ),
        Refusal::Running => "같은 일이 이미 돌고 있습니다".to_string(),
        Refusal::Fresh { age_ms } => {
            format!("같은 일을 {}분 전에 마쳤습니다", minutes(*age_ms))
        }
        Refusal::Concurrent { active, limit } => {
            format!("동시에 도는 호출이 {active}개라 기다립니다 (한도 {limit}개)")
        }
        Refusal::Hourly { used, limit, .. } => {
            format!("지난 한 시간에 {used}번 불렀습니다 (한도 {limit}번)")
        }
        Refusal::Daily { used, limit, .. } => {
            format!("지난 하루에 {used}번 불렀습니다 (한도 {limit}번)")
        }
    }
}

#[cfg(test)]
mod tests;
