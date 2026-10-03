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
fn fnv1a(bytes: impl Iterator<Item = u8>) -> u64 {
    bytes.fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}

/// The identity of a job: the repository it is about, the commit it reads, what
/// it is for and the configuration it runs under. Two launches with the same
/// key are the same job.
#[must_use]
pub fn job_key(repo: &str, head: &str, job: &str, config: &str) -> String {
    let bytes = [repo, head, job, config]
        .into_iter()
        .flat_map(|part| part.bytes().chain(std::iter::once(PART_END)));
    format!("{:016x}", fnv1a(bytes))
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

/// A launch that holds a place.
#[derive(Debug)]
struct Held {
    id: u64,
    at_ms: i64,
    job: Option<String>,
}

/// The ledger: launch stamps, the walls that stand, the jobs that finished
/// and the launches running. Only the first three outlive the process — a
/// running launch does not survive a window that restarted, and its place is
/// not kept for it.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LaunchLedger {
    /// When each launch of the last day was let through, oldest first.
    launches: VecDeque<i64>,
    /// Provider → when its wall stops standing.
    rests: BTreeMap<String, i64>,
    /// Job key → when its last run finished well.
    done: BTreeMap<String, i64>,
    #[serde(skip)]
    active: Vec<Held>,
    #[serde(skip)]
    next_id: u64,
}

impl LaunchLedger {
    /// Asks to start a launch. The first refusal that applies is the answer, in
    /// the order a person would want it told: a resting provider, then the
    /// same job, then the three ceilings.
    ///
    /// # Errors
    ///
    /// The [`Refusal`] that stopped it; nothing was recorded.
    pub fn reserve(
        &mut self,
        ask: &Ask<'_>,
        limits: &Limits,
        now_ms: i64,
    ) -> Result<Permit, Refusal> {
        self.prune(now_ms);
        if let Some(until_ms) = self.rests.get(ask.provider) {
            return Err(Refusal::Resting {
                until_ms: *until_ms,
            });
        }
        if let Some(job) = ask.job {
            if self
                .active
                .iter()
                .any(|held| held.job.as_deref() == Some(job))
            {
                return Err(Refusal::Running);
            }
            if let Some(fresh_ms) = ask.fresh_ms
                && let Some(done_ms) = self.done.get(job)
                && now_ms.saturating_sub(*done_ms) < fresh_ms
            {
                return Err(Refusal::Fresh {
                    age_ms: now_ms.saturating_sub(*done_ms),
                });
            }
        }
        let active = count(self.active.len());
        if let Some(limit) = limits.concurrent
            && active >= limit
        {
            return Err(Refusal::Concurrent { active, limit });
        }
        if let Some(limit) = limits.per_hour {
            let used = self.launched_within(now_ms, MS_PER_HOUR);
            if used >= limit {
                return Err(Refusal::Hourly {
                    used,
                    limit,
                    retry_at_ms: self.frees_at(MS_PER_HOUR, limit),
                });
            }
        }
        if let Some(limit) = limits.per_day {
            let used = self.launched_within(now_ms, MS_PER_DAY);
            if used >= limit {
                return Err(Refusal::Daily {
                    used,
                    limit,
                    retry_at_ms: self.frees_at(MS_PER_DAY, limit),
                });
            }
        }
        self.launches.push_back(now_ms);
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.active.push(Held {
            id,
            at_ms: now_ms,
            job: ask.job.map(str::to_string),
        });
        Ok(Permit {
            id,
            provider: ask.provider.to_string(),
            job: ask.job.map(str::to_string),
            fresh: ask.fresh_ms.is_some(),
        })
    }

    /// A launch ended. Its place is freed — the hourly and daily counts keep
    /// it, because they count launches and not completions — and what it
    /// ended in is remembered: a wall rests its provider, a good run of a job
    /// whose caller reuses answers stands as one.
    pub fn finish(&mut self, permit: Permit, outcome: Outcome, now_ms: i64) {
        self.active.retain(|held| held.id != permit.id);
        match outcome {
            Outcome::Done => {
                if permit.fresh
                    && let Some(job) = permit.job
                {
                    self.done.insert(job, now_ms);
                    self.trim_done();
                }
            }
            Outcome::Failed => {}
            Outcome::Rested { until_ms } => {
                let standing = self.rests.entry(permit.provider).or_insert(until_ms);
                *standing = (*standing).max(until_ms);
            }
        }
    }

    /// What the ledger holds at `now_ms`.
    #[must_use]
    pub fn counters(&self, now_ms: i64) -> Counters {
        Counters {
            active: count(self.active.len()),
            last_hour: self.launched_within(now_ms, MS_PER_HOUR),
            last_day: self.launched_within(now_ms, MS_PER_DAY),
            resting: self
                .rests
                .iter()
                .filter(|(_, until_ms)| **until_ms > now_ms)
                .map(|(provider, until_ms)| Resting {
                    provider: provider.clone(),
                    until_ms: *until_ms,
                })
                .collect(),
        }
    }

    /// Lets go of what has aged out: launches past a day, walls that stopped
    /// standing, jobs past a day, places held by a launch that never ended.
    fn prune(&mut self, now_ms: i64) {
        while self
            .launches
            .front()
            .is_some_and(|at_ms| now_ms.saturating_sub(*at_ms) >= MS_PER_DAY)
            || self.launches.len() > LAUNCHES_KEPT_MAX
        {
            self.launches.pop_front();
        }
        self.rests.retain(|_, until_ms| *until_ms > now_ms);
        self.done
            .retain(|_, done_ms| now_ms.saturating_sub(*done_ms) < MS_PER_DAY);
        self.active
            .retain(|held| now_ms.saturating_sub(held.at_ms) < PERMIT_STALE_MS);
    }

    /// Launches let through in the last `span_ms`.
    fn launched_within(&self, now_ms: i64, span_ms: i64) -> u32 {
        count(
            self.launches
                .iter()
                .rev()
                .take_while(|at_ms| now_ms.saturating_sub(**at_ms) < span_ms)
                .count(),
        )
    }

    /// When a window of `span_ms` holding `limit` launches has a place again:
    /// when the `limit`-th newest ages out. `None` for a ceiling of nothing.
    fn frees_at(&self, span_ms: i64, limit: u32) -> Option<i64> {
        let nth = usize::try_from(limit).ok()?.checked_sub(1)?;
        self.launches
            .iter()
            .rev()
            .nth(nth)
            .map(|at_ms| at_ms.saturating_add(span_ms))
    }

    /// Keeps the fresh rule's memory to [`DONE_KEPT_MAX`] jobs, the oldest out.
    fn trim_done(&mut self) {
        while self.done.len() > DONE_KEPT_MAX {
            let Some(oldest) = self
                .done
                .iter()
                .min_by_key(|(_, done_ms)| **done_ms)
                .map(|(job, _)| job.clone())
            else {
                break;
            };
            self.done.remove(&oldest);
        }
    }
}

/// A length as the counts speak it, saturating where a count cannot.
fn count(length: usize) -> u32 {
    u32::try_from(length).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
