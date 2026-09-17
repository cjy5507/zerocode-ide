//! Jitter applied to computed retry backoff.
//!
//! Pure exponential backoff makes every client that hits the same 429 wake up
//! at the same instant and collide again — a thundering herd that keeps the
//! shared rate bucket saturated. We spread our *own* computed backoff by a
//! deterministic per-thread jitter so N parallel sub-agents de-correlate their
//! retries. A server-provided `Retry-After` is authoritative and is used
//! verbatim by callers — it never reaches this module.
//!
//! The strategy (per-thread LCG, multiplicative band) and ratio were chosen by
//! the simulation in `tests/retry_jitter_bench.rs`, where it beat wall-clock
//! sub-second-nanos jitter and matched AWS decorrelated jitter while staying
//! allocation- and lock-free. The randomness source is a thread-local
//! splitmix64 stream seeded once per thread, so it needs no `rand` dependency
//! and no wall-clock read (both of which would break determinism in tests).

use std::cell::Cell;
use std::time::Duration;

/// Half-width of the multiplicative jitter band: each backoff is scaled by a
/// factor drawn uniformly from `[1 - RATIO, 1 + RATIO]`.
const JITTER_RATIO: f64 = 0.5;

/// One splitmix64 step → uniform value in `[0, 1)`.
fn next_unit(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // Top 53 bits → an f64 mantissa's worth of precision in `[0, 1)`.
    #[allow(clippy::cast_precision_loss)]
    let unit = (z >> 11) as f64 / ((1_u64 << 53) as f64);
    unit
}

/// Scale `base` by a jitter factor in `[1 - RATIO, 1 + RATIO]`, advancing the
/// caller's PRNG `state`. Pure (no thread-local, no clock) so it is directly
/// unit-testable; `spread_backoff` is the thread-local wrapper used in prod.
fn spread(base: Duration, state: &mut u64) -> Duration {
    let base_us = u64::try_from(base.as_micros()).unwrap_or(u64::MAX);
    if base_us == 0 {
        return Duration::ZERO;
    }
    let factor = 1.0 - JITTER_RATIO + next_unit(state) * (JITTER_RATIO * 2.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let jittered = (base_us as f64 * factor.max(0.0)) as u64;
    Duration::from_micros(jittered)
}

/// Seed derived from the current thread's identity so two threads draw
/// independent jitter streams. Deterministic per thread (no clock), which is
/// what de-correlates parallel retries without a `rand` dependency.
fn thread_seed() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    hasher.finish()
}

/// Spread a computed backoff by deterministic per-thread jitter so parallel
/// clients retrying the same throttle don't re-collide. Pass only *our own*
/// exponential backoff here — never a server `Retry-After`.
pub(crate) fn spread_backoff(base: Duration) -> Duration {
    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0) };
    }
    STATE.with(|cell| {
        let mut state = cell.get();
        if state == 0 {
            state = thread_seed();
            if state == 0 {
                state = 0xDEAD_BEEF;
            }
        }
        let out = spread(base, &mut state);
        cell.set(state);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::{JITTER_RATIO, next_unit, spread, spread_backoff};
    use std::collections::HashSet;
    use std::time::Duration;

    #[test]
    fn next_unit_stays_in_unit_interval() {
        let mut state = 0x1234_5678_9ABC_DEF0;
        for _ in 0..100_000 {
            let u = next_unit(&mut state);
            assert!((0.0..1.0).contains(&u), "unit out of range: {u}");
        }
    }

    #[test]
    fn spread_stays_within_ratio_band() {
        let base = Duration::from_millis(500);
        let lo = base.mul_f64(1.0 - JITTER_RATIO);
        let hi = base.mul_f64(1.0 + JITTER_RATIO);
        let mut state = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..100_000 {
            let d = spread(base, &mut state);
            assert!(d >= lo && d <= hi, "{d:?} outside [{lo:?}, {hi:?}]");
        }
    }

    #[test]
    fn spread_zero_base_is_zero() {
        let mut state = 1;
        assert_eq!(spread(Duration::ZERO, &mut state), Duration::ZERO);
    }

    #[test]
    fn spread_de_correlates_successive_draws() {
        // The whole point: consecutive draws from one stream must differ, or
        // parallel retries would not actually spread out.
        let base = Duration::from_millis(500);
        let mut state = 0xDEAD_BEEF_CAFE_F00D;
        let mut seen = HashSet::new();
        for _ in 0..200 {
            seen.insert(spread(base, &mut state).as_micros());
        }
        assert!(
            seen.len() > 100,
            "jitter not spreading: {} unique",
            seen.len()
        );
    }

    #[test]
    fn spread_backoff_entry_is_in_band() {
        let base = Duration::from_millis(500);
        let d = spread_backoff(base);
        assert!(
            d >= base.mul_f64(1.0 - JITTER_RATIO) && d <= base.mul_f64(1.0 + JITTER_RATIO),
            "{d:?} out of band"
        );
    }
}
