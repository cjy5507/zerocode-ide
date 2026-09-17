//! Simulation-based benchmark that compares retry-backoff jitter strategies
//! under the exact pathology we hit in production: N parallel sub-agents
//! all receiving 429 in the same instant and competing for a shared token
//! bucket.
//!
//! Each strategy is run against the same scenario; we measure the wall-clock
//! time until every client has succeeded once. Lower is better.
//!
//! Run with:
//! ```bash
//! cargo test --release --test retry_jitter_bench -- --nocapture
//! ```
//! The `--ignored` filter skips it from the default `cargo test` so CI stays
//! fast; we want it explicit because it does take ~1s of simulated time.

use std::cell::Cell;
use std::sync::{Arc, Mutex};

/// Simulated wall clock — we don't actually sleep, we just advance the model.
type SimInstant = u64; // microseconds since start.

/// A shared token bucket: emits one token every `interval_us`.
struct Bucket {
    interval_us: u64,
    next_available: Mutex<SimInstant>,
}

impl Bucket {
    fn new(interval_us: u64) -> Self {
        Self {
            interval_us,
            next_available: Mutex::new(0),
        }
    }

    /// Try to consume a token at `now`. Returns `Ok(())` if successful,
    /// otherwise `Err(retry_at)` indicating when the next token frees.
    fn try_consume(&self, now: SimInstant) -> Result<(), SimInstant> {
        let mut next = self.next_available.lock().unwrap();
        if now >= *next {
            *next = now + self.interval_us;
            Ok(())
        } else {
            Err(*next)
        }
    }
}

trait JitterStrategy: Sync {
    fn jitter(&self, base_us: u64, thread_id: usize, attempt: u32) -> u64;
}

/// No jitter: pure deterministic exponential backoff.
struct NoJitter;
impl JitterStrategy for NoJitter {
    fn jitter(&self, base_us: u64, _: usize, _: u32) -> u64 {
        base_us
    }
}

/// Wall-clock subsec nanos jitter (the buggy original).
struct WallClockJitter;
impl JitterStrategy for WallClockJitter {
    fn jitter(&self, base_us: u64, _: usize, _: u32) -> u64 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let factor = 0.75 + (f64::from(nanos) / 1_000_000_000.0) * 0.5;
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let v = ((base_us as f64) * factor) as u64;
        v
    }
}

/// Per-thread LCG jitter — what we're shipping.
struct PerThreadLcgJitter {
    ratio: f64,
}
impl JitterStrategy for PerThreadLcgJitter {
    fn jitter(&self, base_us: u64, thread_id: usize, attempt: u32) -> u64 {
        thread_local! {
            static STATE: Cell<u64> = const { Cell::new(0) };
        }
        STATE.with(|cell| {
            let mut state = cell.get();
            if state == 0 {
                state = (thread_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    ^ u64::from(attempt).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                if state == 0 {
                    state = 0xDEAD_BEEF;
                }
            }
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            cell.set(state);
            #[allow(clippy::cast_precision_loss)]
            let unit = (z >> 11) as f64 / ((1_u64 << 53) as f64);
            let factor = 1.0 - self.ratio + unit * (self.ratio * 2.0);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let out = ((base_us as f64) * factor.max(0.0)) as u64;
            out
        })
    }
}

/// Decorrelated jitter (AWS recommended): `next = rand(base, prev * 3)`.
struct DecorrelatedJitter {
    state: Mutex<Vec<u64>>, // last sleep per thread
}
impl DecorrelatedJitter {
    fn new(threads: usize) -> Self {
        Self {
            state: Mutex::new(vec![0; threads]),
        }
    }
}
impl JitterStrategy for DecorrelatedJitter {
    fn jitter(&self, base_us: u64, thread_id: usize, _: u32) -> u64 {
        let mut state = self.state.lock().unwrap();
        let prev = state[thread_id].max(base_us);
        // Cheap PRNG seeded from thread_id and prev.
        let mut z = prev.wrapping_add((thread_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let upper = prev.saturating_mul(3).max(base_us + 1);
        let next = base_us + (z % (upper - base_us));
        state[thread_id] = next;
        next
    }
}

/// Run one scenario: N parallel clients each need to land 1 successful
/// request through a shared 1-rps bucket. Returns total wall-time (us)
/// until all succeed.
fn simulate(
    strategy: &dyn JitterStrategy,
    n_clients: usize,
    bucket_interval_us: u64,
    base_backoff_us: u64,
    max_backoff_us: u64,
    max_attempts: u32,
) -> u64 {
    let bucket = Arc::new(Bucket::new(bucket_interval_us));
    // Sorted event queue: (when, thread_id, attempt).
    let mut events: Vec<(SimInstant, usize, u32)> = (0..n_clients).map(|i| (0, i, 1)).collect();
    let mut succeeded = vec![false; n_clients];
    let mut last_success_time = 0_u64;
    let mut total_attempts = 0_u64;

    while events.iter().any(|(_, tid, _)| !succeeded[*tid]) {
        // Pop earliest event.
        events.sort_by_key(|e| e.0);
        let Some((now, thread_id, attempt)) = events.iter().copied().next() else {
            break;
        };
        events.remove(0);

        if succeeded[thread_id] {
            continue;
        }
        total_attempts += 1;
        if attempt > max_attempts {
            // Give up — treat as failure for scoring.
            last_success_time = last_success_time.max(now);
            succeeded[thread_id] = true;
            continue;
        }

        match bucket.try_consume(now) {
            Ok(()) => {
                succeeded[thread_id] = true;
                last_success_time = last_success_time.max(now);
            }
            Err(retry_at) => {
                // Exponential backoff with strategy's jitter, then sleep
                // at least until `retry_at`.
                let exp = base_backoff_us.saturating_mul(1_u64 << (attempt - 1));
                let capped = exp.min(max_backoff_us);
                let jitter = strategy.jitter(capped, thread_id, attempt);
                let wake_at = (now + jitter).max(retry_at);
                events.push((wake_at, thread_id, attempt + 1));
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let finished_ms = last_success_time as f64 / 1000.0;
    eprintln!(
        "  total_attempts={total_attempts}  finished_at={last_success_time}us  ({finished_ms:.1}ms)"
    );
    last_success_time
}

#[test]
#[ignore = "runs ~10s of simulated scenarios; explicit opt-in"]
fn bench_jitter_strategies() {
    // Scenario: 8 parallel sub-agents, shared 5-rps bucket
    // (200ms between tokens), base backoff 500ms, max 30s.
    const N: usize = 8;
    const BUCKET_US: u64 = 200_000;
    const BASE_US: u64 = 500_000;
    const MAX_US: u64 = 30_000_000;
    const MAX_ATTEMPTS: u32 = 10;

    eprintln!(
        "=== {N} parallel clients, 1 token / {}ms ===",
        BUCKET_US / 1000
    );

    eprintln!("\n[strategy] none (deterministic)");
    let _t_none = simulate(&NoJitter, N, BUCKET_US, BASE_US, MAX_US, MAX_ATTEMPTS);

    eprintln!("\n[strategy] wall-clock subsec nanos");
    let _t_wall = simulate(
        &WallClockJitter,
        N,
        BUCKET_US,
        BASE_US,
        MAX_US,
        MAX_ATTEMPTS,
    );

    for ratio in [0.10, 0.25, 0.35, 0.40, 0.45, 0.50, 0.60, 0.75] {
        eprintln!("\n[strategy] per-thread LCG @ ratio={ratio}");
        let _t = simulate(
            &PerThreadLcgJitter { ratio },
            N,
            BUCKET_US,
            BASE_US,
            MAX_US,
            MAX_ATTEMPTS,
        );
    }

    eprintln!("\n[strategy] decorrelated (AWS)");
    let _t_dec = simulate(
        &DecorrelatedJitter::new(N),
        N,
        BUCKET_US,
        BASE_US,
        MAX_US,
        MAX_ATTEMPTS,
    );

    eprintln!("\n=== higher pressure: 16 clients, 250ms bucket ===");
    let pressure_n = 16;
    let pressure_us = 250_000;
    for ratio in [0.25, 0.35, 0.40, 0.45, 0.50, 0.60] {
        eprintln!("\n[strategy] per-thread LCG @ ratio={ratio}");
        let _t = simulate(
            &PerThreadLcgJitter { ratio },
            pressure_n,
            pressure_us,
            BASE_US,
            MAX_US,
            MAX_ATTEMPTS,
        );
    }
    eprintln!("\n[strategy] decorrelated (AWS) under pressure");
    let _t = simulate(
        &DecorrelatedJitter::new(pressure_n),
        pressure_n,
        pressure_us,
        BASE_US,
        MAX_US,
        MAX_ATTEMPTS,
    );
}
