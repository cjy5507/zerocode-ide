//! The one ledger every background launch of an agent's CLI passes through
//! (t-26583 — ruflo's #2661 `global-ai-budget` and `ai-job-dedup`, read
//! against this window).
//!
//! A pane belongs to a person or to a worker. This is about the other
//! launches: the headless runs the window starts for itself — a commit draft,
//! a Computer Use value, a login renewal — which nobody watches, which spend
//! the person's plan, and which a loop can repeat thousands of times before
//! anyone looks. Each of them asks here BEFORE a process exists, and the answer
//! is the whole policy:
//!
//! - **A provider that answered 429 rests.** One launch meeting a wall closes
//!   the door for every launch on that provider, whichever window or feature
//!   asks, until the wall's time ([`Outcome::Rested`]). Another provider's door
//!   stays open: Claude's wall does not stop Codex.
//! - **Three ceilings, user-wide** ([`Limits`]): at once, per hour, per day —
//!   counted over sliding windows, so a ceiling never resets at a clock edge
//!   and a restart cannot zero it (the ledger is serde; the window keeps it).
//! - **The same job runs once** ([`job_key`]): a launch naming the job it is
//!   (repository, commit, what for, configuration) is refused while that job
//!   runs, and — for a caller that can reuse the answer — while a finished run
//!   still stands as it ([`Ask::fresh_ms`]).
//!
//! Nothing here reads a clock, a file or a process: the time arrives as an
//! argument and the state is a value, so every rule below is a test and not a
//! hope.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::civil::{MS_PER_DAY, MS_PER_HOUR};

/// Launches that may run at once, user-wide. Four: every one-shot the window
/// starts is held to a wall of a minute or less
/// ([`crate::commit_message::GENERATION_TIMEOUT`]), so more than four at a time
/// is a fan-out and not a person's hands.
pub const DEFAULT_CONCURRENT: u32 = 4;

/// Launches per hour — three hundred, five a minute for an hour. A person's
/// own button is a few a minute at the very most, and a Computer Use walk asks
/// a value once per field it meets; a loop of one-shots that fail at about a
/// second apiece reaches the ceiling in five minutes. A first estimate: the
/// counters on the settings card ([`LaunchLedger::counters`]) are what it is
/// calibrated against after a week of real use.
pub const DEFAULT_PER_HOUR: u32 = 300;

/// Launches per day — two thousand, about seven hours of the hourly ceiling
/// held without a break. An honest day sits far under; a loop left running
/// overnight does not.
pub const DEFAULT_PER_DAY: u32 = 2_000;

/// How long a permit that was never finished still holds a place: a window
/// that crashed between asking and finishing leaves one, and a place held
/// forever is a ceiling that tightens until nothing launches. Thirty minutes
/// is thirty times the longest wall a one-shot runs under.
pub const PERMIT_STALE_MS: i64 = 30 * 60 * 1_000;

/// How long a provider rests when its wall named no reset — one gauge refetch
/// floor and a half: long enough that the next launch is not the same
/// refusal again at once, short enough that a wall that lifted early costs
/// the person half an hour and not the six a worker's wait is bounded by.
pub const REST_UNKNOWN_MS: i64 = 30 * 60 * 1_000;

/// Finished jobs remembered for the fresh rule. A job is one key of sixteen
/// characters; two hundred and fifty-six of them are four kilobytes, and the
/// oldest go first.
pub const DONE_KEPT_MAX: usize = 256;

/// Launch stamps remembered for the hourly and daily counts. A ledger with no
/// daily ceiling would otherwise keep every launch of a day; past this the
/// oldest stamps go and the counts read low, never high.
pub const LAUNCHES_KEPT_MAX: usize = 20_000;

/// FNV-1a's 64-bit offset basis and prime. The job key is persisted and compared
/// across runs, so it uses a hash whose output is specified, not std's, whose
/// algorithm is free to change with the next compiler.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// What ends one part of a job key. The ASCII unit separator: no repository
/// path, commit id, purpose or configuration text carries it, so `("ab", "c")`
/// and `("a", "bc")` are two jobs.
const PART_END: u8 = 0x1f;

/// The person's three ceilings; `None` is no ceiling on that axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub concurrent: Option<u32>,
    pub per_hour: Option<u32>,
    pub per_day: Option<u32>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            concurrent: Some(DEFAULT_CONCURRENT),
            per_hour: Some(DEFAULT_PER_HOUR),
            per_day: Some(DEFAULT_PER_DAY),
        }
    }
}

/// FNV-1a over a byte stream.
fn fnv1a(_bytes: impl Iterator<Item = u8>) -> u64 {
    0
}

/// The identity of a job.
#[must_use]
pub fn job_key(_repo: &str, _head: &str, _job: &str, _config: &str) -> String {
    String::new()
}

/// One launch asking to start.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    /// Whose plan it spends — the gauge's own name (`claude`, `codex`) — and
    /// so whose wall it is refused by.
    pub provider: &'a str,
    /// The job, by [`job_key`]; `None` for a launch nothing else is the same as.
    pub job: Option<&'a str>,
    /// How long a finished run of the job still stands as its answer, for a
    /// caller that can reuse one; `None` refuses a repeat only while the job
    /// runs.
    pub fresh_ms: Option<i64>,
}

/// A place held by a launch that was let through. Finished with
/// [`LaunchLedger::finish`]; one that never is lapses ([`PERMIT_STALE_MS`]).
#[derive(Debug)]
pub struct Permit {
    id: u64,
    provider: String,
    job: Option<String>,
    fresh: bool,
}

/// How a launch ended, as far as the ledger needs to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It ran and answered.
    Done,
    /// It ran and failed for a reason that is not a wall: the next launch is
    /// not refused for it.
    Failed,
    /// Its provider answered with a wall that stands until `until_ms`.
    Rested { until_ms: i64 },
}

/// Why a launch was not let through, with what the caller needs to say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The provider answered a wall and has not been let back in.
    Resting { until_ms: i64 },
    /// The same job is running.
    Running,
    /// The same job finished `age_ms` ago and its answer still stands.
    Fresh { age_ms: i64 },
    /// `active` launches are running and the ceiling is `limit`.
    Concurrent { active: u32, limit: u32 },
    /// `used` launches in the last hour and the ceiling is `limit`; the hour
    /// frees a place at `retry_at_ms` (`None` for a ceiling of nothing).
    Hourly {
        used: u32,
        limit: u32,
        retry_at_ms: Option<i64>,
    },
    /// The same for the last day.
    Daily {
        used: u32,
        limit: u32,
        retry_at_ms: Option<i64>,
    },
}

impl Refusal {
    /// The word a row or a toast keys its sentence on.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Resting { .. } => "resting",
            Self::Running => "running",
            Self::Fresh { .. } => "fresh",
            Self::Concurrent { .. } => "concurrent",
            Self::Hourly { .. } => "hourly",
            Self::Daily { .. } => "daily",
        }
    }
}

/// A provider the ledger holds a wall for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Resting {
    pub provider: String,
    pub until_ms: i64,
}

/// What the ledger holds now, for the settings card to say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Counters {
    pub active: u32,
    pub last_hour: u32,
    pub last_day: u32,
    pub resting: Vec<Resting>,
}

/// The ledger: launch stamps, the walls that stand, the jobs that finished
/// and the launches running.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LaunchLedger {
    launches: VecDeque<i64>,
    rests: BTreeMap<String, i64>,
    done: BTreeMap<String, i64>,
}

impl LaunchLedger {
    /// Asks to start a launch.
    ///
    /// # Errors
    ///
    /// The [`Refusal`] that stopped it; nothing was recorded.
    pub fn reserve(
        &mut self,
        ask: &Ask<'_>,
        _limits: &Limits,
        _now_ms: i64,
    ) -> Result<Permit, Refusal> {
        Ok(Permit {
            id: 0,
            provider: ask.provider.to_string(),
            job: ask.job.map(str::to_string),
            fresh: ask.fresh_ms.is_some(),
        })
    }

    /// A launch ended.
    pub fn finish(&mut self, _permit: Permit, _outcome: Outcome, _now_ms: i64) {}

    /// What the ledger holds at `now_ms`.
    #[must_use]
    pub fn counters(&self, _now_ms: i64) -> Counters {
        Counters {
            active: 0,
            last_hour: 0,
            last_day: 0,
            resting: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
