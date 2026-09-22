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
//! whether it arrives or not. And the second copy is bounded by that same
//! wall, measured from the *first* request's own first byte
//! (`SystemOneClient::attempts`) — so what the second copy really gets is
//! `wall - delay`, its **room**, and a delay chosen only from `maxExtraLoad`
//! can leave no room worth having.
//!
//! So the delay is the load's to name and the room is the wall's to refuse:
//!
//! ```text
//! delay = percentile(1 - MAX_EXTRA_LOAD)
//! room  = wall - delay,   and no hedge unless room >= ANSWERS_OF_ROOM * median
//! ```
//!
//! # What the ledger said when it was asked (2026-09-22, t-5874)
//!
//! The rule shipped with the two terms folded into `min(percentile,
//! wall - median)` instead, which is the same formula with the room silently
//! fixed at one median — and which lets the wall pull the delay *earlier*
//! than the load rank, past the preference `MAX_EXTRA_LOAD` states. Both
//! halves of that are now measured, on every hedge this machine has fired:
//! 68 of them, 5 on the routing ledger and 63 on recall.
//!
//! | | firings | second copy won | first copy won | both died at the wall | load p50 / max |
//! |---|---|---|---|---|---|
//! | the folded rule | 68 | 11 | 43 | 14 | 0.22 / **0.39** |
//! | this rule | 41 | 10 | 24 | 7 | 0.22 / 0.25 |
//!
//! Three readings, in the order they change the rule:
//!
//! 1. **A copy with one median of room does not land.** Sorted by the room
//!    the copy was given, the firings split cleanly: one median of room or
//!    less won 1 of 27, two or more won 10 of 41. `ANSWERS_OF_ROOM` is that
//!    cliff and nothing else — a sweep of it over these same 68 firings puts
//!    the requests spent per expected rescue at 8.0 (one median), 7.2 (1.75),
//!    **6.1 (two)**, 8.0 (2.5).
//! 2. **The folded rule broke its own preference.** The `min` let the wall
//!    name a delay earlier than the load rank on 6 of the 68, where the share
//!    of calls leaving twice reached 0.39 against the 0.25 this module
//!    promises. Reading the delay from the load rank alone bounds it by
//!    construction: 0.25 is now the measured maximum, not an aspiration.
//! 3. **The joint latency distribution is not observed.** Fourteen races
//!    timed out with both copies still unanswered, and eleven were won by
//!    the second copy without the first copy's eventual latency. A timeout
//!    already means neither copy answered: selecting those fourteen cannot
//!    test independence or count rescues among the eleven wins. One other
//!    race recorded both latencies, 1,397 ms and 1,398 ms; one pair is not a
//!    distribution. [`cleared_share`] therefore remains an independent-draw
//!    estimate, not a measured rescue rate or a statistical upper bound.
//!
//! The room rule is supported here by the replay's observed race outcomes.
//! Its effect on uncensored completion latencies still needs measurement.

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
///
/// Since t-5874 it is also what it says: the delay is this rank, so the share
/// that leaves twice cannot exceed it. The rule it replaced could, and did,
/// on 6 of this machine's 68 firings.
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

/// How many ordinary answers must fit in the second copy's own budget
/// ([`HedgePlan::room`]) before it is worth sending.
///
/// The second copy does not get the wall; it gets what is left of the wall
/// after the delay, because both copies are bounded from the first request's
/// first byte. Two, because this machine's 68 firings say so and say it
/// sharply: given one median of room or less the copy answered first on 1 of
/// 27, given two or more on 10 of 41. The cliff is between one and two, so
/// the constant is two — a copy that is not given as long as two ordinary
/// answers is being sent to arrive late, at full price.
///
/// A median rather than a higher rank because the copy is a fresh draw, not
/// the first copy's remainder, and the median is the draw it most likely
/// takes.
pub const ANSWERS_OF_ROOM: u64 = 2;

/// A judgment may leave at most twice: the first request and one hedge. So a
/// hedge's cost is bounded at twice the requests by construction, whatever
/// the distribution does, and no budget can be surprised by a third.
pub const MAX_ATTEMPTS: u32 = 2;

/// When a second request leaves, how long it then has, and what that costs on
/// the samples the delay was read from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HedgePlan {
    /// How long after the first request the second one leaves.
    pub delay: Duration,
    /// What the second copy has to answer in: the wall less the delay, since
    /// both copies are bounded from the first request's own first byte. The
    /// number [`ANSWERS_OF_ROOM`] is measured against, worth carrying because
    /// it — not the delay — is what decided whether this plan exists.
    pub room: Duration,
    /// The share of `samples` that ran past `delay`: the requests that would
    /// actually have left twice. This is what the plan costs, measured rather
    /// than assumed, and a caller whose budget cannot carry it declines.
    ///
    /// At most [`MAX_EXTRA_LOAD`] by construction, because `delay` is that
    /// rank.
    pub extra_load: f64,
}

/// The delay for a judgment whose answers have looked like `samples`
/// (milliseconds, in any order), against the `wall` past which an answer is
/// discarded.
///
/// `None` — ask once — when the sample is too small to name a rank
/// ([`MIN_SAMPLES`]), when the delay the load prefers is itself past the wall,
/// or when what is left of the wall after it cannot hold
/// [`ANSWERS_OF_ROOM`] ordinary answers. That last one is the whole rule: a
/// second copy is worth its price only where it has room to land, and on this
/// machine's ledger a copy without that room lands 1 time in 27.
#[must_use]
pub fn plan(samples: &[u64], wall: Duration) -> Option<HedgePlan> {
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let wall_ms = u64::try_from(wall.as_millis()).unwrap_or(u64::MAX);
    // What the day's preferred load buys, and nothing else: a delay the wall
    // pulled earlier than this rank would spend more than MAX_EXTRA_LOAD says
    // it prefers to, which is the bug this replaced.
    let delay = percentile(&sorted, 1.0 - MAX_EXTRA_LOAD);
    if delay == 0 || delay >= wall_ms {
        return None;
    }
    // What the wall demands in return: the copy leaves inside the same wall
    // the first request did, so this is all it has.
    let room = wall_ms - delay;
    if room < ANSWERS_OF_ROOM.saturating_mul(percentile(&sorted, 0.5)) {
        return None;
    }
    let over = sorted.iter().filter(|answer| **answer > delay).count();
    Some(HedgePlan {
        delay: Duration::from_millis(delay),
        room: Duration::from_millis(room),
        #[expect(
            clippy::cast_precision_loss,
            reason = "a share of a sample this rule already refuses below MIN_SAMPLES"
        )]
        extra_load: over as f64 / sorted.len() as f64,
    })
}

/// The share of `samples` that would land inside `wall` when a judgment is
/// asked once, and when it is asked again after `plan.delay`.
///
/// The second number treats the two answers as independent draws from the
/// same sample — the assumption every published hedge rests on, and the one
/// the field warns about: two requests that queue behind the same overloaded
/// back-end are slow together, and then a hedge buys less than this says. It
/// was stated here as arithmetic on a sample so a ledger of real hedges could
/// contradict it, which is the only way to find out.
///
/// The ledger does not identify that joint distribution: a winning race
/// usually cancels its loser, and conditioning on a timeout selects races
/// where both copies failed by definition. This is an estimate under the
/// stated assumption, not a forecast or an upper bound for arbitrary
/// dependence. The plan itself is decided by [`HedgePlan::room`].
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
        let second = u64::try_from(plan.room.as_millis()).unwrap_or(0);
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
