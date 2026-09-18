//! When a judgment's second try leaves.
//!
//! A judgment that has not answered yet is not a failed judgment — it is a
//! slow one, and the wire's slowness here is its own, not the request's. The
//! ledger says so: on this machine's routing rows the slowest call, 6,798 ms,
//! carried the *smallest* body of them all (535 tokens), 979 tokens answered
//! in 4,847 ms and 1,038 tokens in 241 ms, and every row retried zero times.
//! Sending less cannot cure that. Asking twice can.
//!
//! # What the field does
//!
//! - **gRPC**'s `hedgingPolicy` takes a `hedgingDelay` written by hand, and
//!   its own guide says to set it from a healthy latency distribution rather
//!   than the mean — which leaves the reader to find that distribution.
//! - **Finagle**'s `BackupRequestFilter` derives the delay instead, and this
//!   is the good idea: one knob, `maxExtraLoad`, and the delay is the
//!   `100 * (1 - maxExtraLoad)`-th percentile of a windowed history, so the
//!   share of requests that leave twice is the knob itself. A retry budget
//!   guards a sudden shift.
//! - The **adaptive** reading of the same design keeps that history in a
//!   DDSketch, rotates two of them on a fixed interval, and refills a token
//!   bucket at `RPS * budget%`.
//!
//! # Why a copy of any of them would be wrong here
//!
//! The adaptive write-up names five places hedging fails. Three of them are
//! this door exactly:
//!
//! - *A service with very little traffic has too few observations for an
//!   accurate quantile.* A day of routing judgments here is tens of calls,
//!   not thousands. So [`MIN_SAMPLES`] refuses to name a percentile from
//!   fewer answers than can carry one, and the answer is no hedge rather than
//!   a guess.
//! - *A rate-limited API pays its quota twice per hedged call.* Jev's door
//!   counts the day's requests against the person's own budget. So the second
//!   request is not free load to be bounded by a fraction — it is the
//!   person's money, and the door's count is the hard bound. `MAX_EXTRA_LOAD`
//!   is only what this rule *prefers* to spend; what it *may* spend is the
//!   day's remaining count, which the caller asks the door for.
//! - *A single back-end adds load to an already-slow instance.* There is one
//!   Jev. A hedge that fires on most calls would be a second wire's worth of
//!   traffic at the same door, so the plan reports the load it expects
//!   ([`HedgePlan::extra_load`]) and a caller that cannot afford it declines.
//!
//! # The one thing the field's rule is missing here
//!
//! Finagle and the sketches lower p99 in general. This door has a harder
//! objective: a judgment is *used* only if it lands inside a wall
//! (`DECISION_ACTIVE_DEADLINE`), and past that wall an answer is discarded
//! whether it arrives or not. A delay chosen only from `maxExtraLoad` can
//! therefore be past the wall, where a second request cannot finish in time
//! and buys nothing at full price — on this machine's samples the
//! load-derived delay is 2,257 ms against a 1,500 ms wall.
//!
//! So the rule takes the earlier of the two:
//!
//! ```text
//! delay = min( percentile(1 - MAX_EXTRA_LOAD),  wall - median )
//! ```
//!
//! The second term is what a wall asks for and what nothing in the field has:
//! leave the second request as long as an ordinary answer takes, or do not
//! bother it. On the 22 answered routing rows that is
//! `min(2257, 1500 - 636) = 864 ms`, where the load-derived delay alone would
//! have been 2,257 ms — past the wall, and worth nothing. A replay of those
//! same rows clears the wall on 81.8% of them against 63.6% asked once, for
//! 1.41x the requests, and this module's tests hold all four numbers to that
//! sample.

use core::time::Duration;

/// The share of judgments this rule *prefers* to let leave twice, and so the
/// percentile it reads the delay from: `1 - MAX_EXTRA_LOAD`.
///
/// A quarter, where Finagle's own guide suggests hundredths, because the
/// sample is tens of calls rather than millions: at 22 answers a hundredth
/// names the single worst outlier and a quarter names a rank the sample can
/// actually support. It is a preference, not a bound — the day's count at the
/// door is the bound, and [`HedgePlan::extra_load`] says what this delay
/// really costs on the samples it was read from.
pub const MAX_EXTRA_LOAD: f64 = 0.25;

/// The fewest answers a percentile may be named from.
///
/// Below this the sample cannot carry a rank: the adaptive reading of this
/// design names "very low traffic services lack sufficient observations for
/// accurate quantiles" as one of the places hedging fails, and a door that
/// answers tens of times a day is that service. Eight is the fewest that puts
/// two answers above the [`MAX_EXTRA_LOAD`] rank, so the delay is a rank the
/// sample holds rather than its own maximum.
pub const MIN_SAMPLES: usize = 8;

/// A judgment may leave at most twice: the first request and one hedge. So a
/// hedge's cost is bounded at twice the requests by construction, whatever
/// the distribution does, and no budget can be surprised by a third.
pub const MAX_ATTEMPTS: u32 = 2;

/// When a second request leaves, and what that costs on the samples the delay
/// was read from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HedgePlan {
    /// How long after the first request the second one leaves.
    pub delay: Duration,
    /// The share of `samples` that ran past `delay`: the requests that would
    /// actually have left twice. This is what the plan costs, measured rather
    /// than assumed, and a caller whose budget cannot carry it declines.
    pub extra_load: f64,
    /// Whether the wall, not [`MAX_EXTRA_LOAD`], set the delay — the case the
    /// field's rule does not have, worth a ledger column because it says the
    /// plan is spending more than it prefers to in order to land in time.
    pub wall_bound: bool,
}

/// The delay for a judgment whose answers have looked like `samples`
/// (milliseconds, in any order), against the `wall` past which an answer is
/// discarded.
///
/// `None` — ask once — when the sample is too small to name a rank
/// ([`MIN_SAMPLES`]), or when an ordinary answer already runs past the wall,
/// because then the whole distribution is late and a second copy of it is
/// late too: that is a wall to raise or a judgment to drop, not a request to
/// duplicate.
#[must_use]
pub fn plan(samples: &[u64], wall: Duration) -> Option<HedgePlan> {
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let wall_ms = u64::try_from(wall.as_millis()).unwrap_or(u64::MAX);
    // What the day's preferred load buys, and what the wall demands: the
    // second request needs as long as an ordinary answer takes.
    let by_load = percentile(&sorted, 1.0 - MAX_EXTRA_LOAD);
    let by_wall = wall_ms.checked_sub(percentile(&sorted, 0.5))?;
    let delay = by_load.min(by_wall);
    if delay == 0 || delay >= wall_ms {
        return None;
    }
    let over = sorted.iter().filter(|answer| **answer > delay).count();
    Some(HedgePlan {
        delay: Duration::from_millis(delay),
        #[expect(
            clippy::cast_precision_loss,
            reason = "a share of a sample this rule already refuses below MIN_SAMPLES"
        )]
        extra_load: over as f64 / sorted.len() as f64,
        wall_bound: by_wall < by_load,
    })
}

/// The share of `samples` that would land inside `wall` when a judgment is
/// asked once, and when it is asked again after `plan.delay`.
///
/// The second number treats the two answers as independent draws from the
/// same sample — the assumption every published hedge rests on, and the one
/// the field warns about: two requests that queue behind the same overloaded
/// back-end are slow together, and then a hedge buys less than this says. It
/// is stated here as arithmetic on a sample so a ledger of real hedges can
/// contradict it, which is the only way to find out.
#[must_use]
pub fn cleared_share(samples: &[u64], wall: Duration, plan: Option<&HedgePlan>) -> (f64, f64) {
    let wall_ms = u64::try_from(wall.as_millis()).unwrap_or(u64::MAX);
    let share = |limit: u64| {
        #[expect(clippy::cast_precision_loss, reason = "a share of a bounded sample")]
        let share = samples.iter().filter(|answer| **answer <= limit).count() as f64
            / samples.len().max(1) as f64;
        share
    };
    let once = share(wall_ms);
    let twice = plan.map_or(once, |plan| {
        let second = wall_ms.saturating_sub(u64::try_from(plan.delay.as_millis()).unwrap_or(0));
        1.0 - (1.0 - once) * (1.0 - share(second))
    });
    (once, twice)
}

/// The `q`-th value of a sorted, non-empty sample by nearest rank — the
/// reading that always names a value the sample actually holds, so a delay is
/// never a number no answer ever took.
fn percentile(sorted: &[u64], q: f64) -> u64 {
    debug_assert!(!sorted.is_empty(), "a percentile of nothing");
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a rank inside a sample this module refuses below MIN_SAMPLES"
    )]
    let rank = (q * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

#[cfg(test)]
mod tests;
