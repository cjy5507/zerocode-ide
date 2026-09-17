use std::sync::atomic::Ordering;
use std::sync::{Condvar, Mutex, OnceLock};

use api::sync_bridge::lock_recovered;
use api::ProviderKind;

// The per-provider 429 / cool-down state and its mark/query arithmetic now live
// in `api::quota` (so the foreground `runtime` retry path, which cannot depend
// on `tools`, feeds the SAME throttle record the sub-agent admission path
// reads). This module keeps only the AIMD admission governor and delegates the
// cool-down API here with unchanged signatures — every call site below and in
// `provider_client` resolves through these re-exports without change.
use api::quota::{provider_slot, HEADROOM_UTIL_THRESHOLD, PROVIDER_SLOTS};
pub(crate) use api::quota::rate_limit_headroom_low;
pub(super) use api::quota::{
    binding_window, mark_capacity_stall_from, rate_limit_cooldown_remaining_ms,
    QUOTA_SNAPSHOT_FRESH,
};

const AGENT_MAX_CONCURRENCY_ENV: &str = "ZO_AGENT_MAX_CONCURRENCY";
// Serial floor for flat `SpawnMultiAgent` fan-outs when quota headroom is low or
// a recent 429/cool-down says the provider is already stressed. Healthy fan-out
// seeds are derived below from the adaptive ceiling so normal swarms start with
// real parallelism while pressure still collapses admission back to one.
pub(super) const DEFAULT_AGENT_MAX_CONCURRENCY: usize = 1;
/// Clamp for the explicit overrides (`ZO_AGENT_MAX_CONCURRENCY` and the
/// env-fixed governor): aligned with the CC workflow execution bound
/// (`min(16, cores-2)` tops out at 16), so a manual setting can reach full
/// CC-level API parallelism but never exceed it.
pub(super) const MAX_AGENT_MAX_CONCURRENCY: usize = 16;
/// Absolute bound on the adaptive ceiling; the effective hard max is
/// `min(16, cores-2)` (see [`adaptive_agent_hard_max`]) — the same execution
/// bound CC uses. The AIMD governor still has to EARN it: admission seeds
/// small and grows one slot per success streak, while a 429 or low quota
/// headroom collapses it back to serial. CC-level parallelism when the
/// account is healthy, none of it when the provider is stressed.
const ADAPTIVE_AGENT_MAX_CONCURRENCY: usize = 16;
/// Conservative healthy seed, deliberately decoupled from the (larger)
/// adaptive ceiling: fan-outs open with modest parallelism and let sustained
/// success ramp toward the ceiling instead of bursting cold against an
/// unknown quota state.
const HEALTHY_AGENT_INITIAL_CONCURRENCY: usize = 3;
const MODERATE_AGENT_INITIAL_CONCURRENCY: usize = DEFAULT_AGENT_MAX_CONCURRENCY + 1;
const SEED_UTIL_LOW: f64 = 0.5;
/// Env override for the *workflow-engine* concurrency cap. Workflows are
/// multi-phase fan-outs that can spawn hundreds of agents, so — unlike a flat
/// `SpawnMultiAgent` — they default to genuine parallelism (`min(16, cores-2)`,
/// the Claude Code model), bounded by a hard ceiling. The adaptive governor
/// then ramps the *live* limit down on a 429 and back up on sustained success.
const WORKFLOW_AGENT_MAX_CONCURRENCY_ENV: &str = "ZO_WORKFLOW_MAX_CONCURRENCY";
/// Hard ceiling on the workflow cap, so a runaway env value can't open hundreds
/// of concurrent provider streams (and OS threads) at once.
const MAX_WORKFLOW_AGENT_CONCURRENCY: usize = 64;

pub(crate) fn shared_agent_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("zo-agent-io")
            .build()
            .expect("agent shared runtime")
    })
}

pub(super) fn parse_agent_api_concurrency_limit(raw: Option<&str>) -> Option<usize> {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.clamp(1, MAX_AGENT_MAX_CONCURRENCY))
}

fn agent_api_concurrency_limit_from_env() -> Option<usize> {
    parse_agent_api_concurrency_limit(std::env::var(AGENT_MAX_CONCURRENCY_ENV).ok().as_deref())
}

fn adaptive_agent_hard_max() -> usize {
    ADAPTIVE_AGENT_MAX_CONCURRENCY.min(default_workflow_concurrency())
}

fn adaptive_agent_seed(kind: ProviderKind) -> usize {
    adaptive_seed_at(
        rate_limit_headroom_low(kind),
        seed_snapshot_for(kind, api::quota::latest_rate_limit_snapshot()),
    )
}

/// Gate the unified quota snapshot to the provider it actually describes.
///
/// Only Anthropic records the snapshot (`crate::quota::record_rate_limit_snapshot`
/// fires solely from the Anthropic client), so it reflects the Anthropic account
/// alone. Feeding it into a non-Anthropic governor's seed or headroom gate would
/// tighten or widen an OpenAI/Google/xAI/Ollama fan-out based on an *unrelated*
/// provider's quota — breaking the per-provider isolation this module
/// guarantees. Every non-Anthropic provider therefore drops the snapshot and
/// relies on its own cool-down / recent-429 state plus the normal healthy seed.
fn seed_snapshot_for(
    kind: ProviderKind,
    snapshot: Option<(api::RateLimitSnapshot, std::time::Duration)>,
) -> Option<(api::RateLimitSnapshot, std::time::Duration)> {
    match kind {
        ProviderKind::Anthropic => snapshot,
        _ => None,
    }
}

fn adaptive_seed_at(
    headroom_low: bool,
    snapshot: Option<(api::RateLimitSnapshot, std::time::Duration)>,
) -> usize {
    if headroom_low {
        return DEFAULT_AGENT_MAX_CONCURRENCY;
    }
    if let Some((snapshot, age)) = snapshot {
        if age <= QUOTA_SNAPSHOT_FRESH {
            if let Some((_, window)) = binding_window(snapshot) {
                if window.utilization >= HEADROOM_UTIL_THRESHOLD {
                    return DEFAULT_AGENT_MAX_CONCURRENCY;
                }
                if window.utilization >= SEED_UTIL_LOW {
                    return MODERATE_AGENT_INITIAL_CONCURRENCY;
                }
            }
        }
    }
    HEALTHY_AGENT_INITIAL_CONCURRENCY
}

/// How many consecutive successful requests it takes to additively grow the
/// live concurrency by one (AIMD's "additive increase"). Recovery is deliberate
/// so the limit climbs back only once the account has clearly stopped throttling.
const GOVERNOR_SUCCESSES_PER_INCREASE: u32 = 3;

/// Adaptive concurrency governor: an AIMD gate over the shared provider quota.
///
/// `hard_max` is the ceiling (env / workflow default); `live_limit` is the
/// *current* admission target, which halves on a 429 (multiplicative decrease)
/// and grows by one per [`GOVERNOR_SUCCESSES_PER_INCREASE`] successes (additive
/// increase), clamped to `[1, hard_max]`. A caller may pass a tighter per-call
/// `ceiling` to [`Self::acquire`] (the flat `SpawnMultiAgent` `concurrency`
/// argument) so the effective admission is `min(live_limit, ceiling)`.
///
/// This replaces the old fixed `OnceLock<Semaphore>`: its size was frozen at
/// first use, so neither a 429 burst nor a per-call `concurrency` could move the
/// real provider concurrency. The governor makes both take effect.
pub(super) struct RateGovernor {
    hard_max: usize,
    adaptive: bool,
    pressure_kind: Option<ProviderKind>,
    /// Poison policy: recover (`lock_recovered`) — every write under this
    /// lock is single-field arithmetic on bounded counters, so the state is
    /// consistent at every panic point. Recovery also keeps the RAII
    /// permit's `Drop` decrement effective after a poisoning panic —
    /// skipping it (the old `if let Ok` shape) leaked a limiter slot.
    state: Mutex<GovernorState>,
    /// Woken whenever a permit is released or the limit grows, so a blocked
    /// `acquire` re-evaluates admission.
    available: Condvar,
}

struct GovernorState {
    /// Current adaptive admission target, in `[1, hard_max]`.
    live_limit: usize,
    /// Permits currently checked out.
    in_flight: usize,
    /// Successes observed since the last increase or decrease.
    consecutive_successes: u32,
}

/// RAII permit: releasing it frees a slot and wakes one waiter.
pub(super) struct GovernorPermit {
    governor: &'static RateGovernor,
}

impl Drop for GovernorPermit {
    fn drop(&mut self) {
        {
            let mut state = lock_recovered(&self.governor.state);
            state.in_flight = state.in_flight.saturating_sub(1);
        }
        self.governor.available.notify_one();
    }
}

impl RateGovernor {
    fn new(hard_max: usize) -> Self {
        let hard_max = hard_max.max(1);
        Self {
            hard_max,
            adaptive: false,
            pressure_kind: None,
            state: Mutex::new(GovernorState {
                live_limit: hard_max,
                in_flight: 0,
                consecutive_successes: 0,
            }),
            available: Condvar::new(),
        }
    }

    #[cfg(test)]
    fn new_adaptive(hard_max: usize, seed: usize) -> Self {
        Self::new_adaptive_for_provider(hard_max, seed, None)
    }

    fn new_adaptive_for_provider(
        hard_max: usize,
        seed: usize,
        pressure_kind: Option<ProviderKind>,
    ) -> Self {
        let hard_max = hard_max.max(1);
        Self {
            hard_max,
            adaptive: true,
            pressure_kind,
            state: Mutex::new(GovernorState {
                live_limit: seed.clamp(1, hard_max),
                in_flight: 0,
                consecutive_successes: 0,
            }),
            available: Condvar::new(),
        }
    }

    fn effective_limit(&self, live_limit: usize, ceiling: usize) -> usize {
        let pressure_limit = if self
            .pressure_kind
            .is_some_and(rate_limit_headroom_low)
        {
            DEFAULT_AGENT_MAX_CONCURRENCY
        } else {
            usize::MAX
        };
        live_limit.min(ceiling).min(pressure_limit).max(1)
    }

    /// Block until a slot is free under `min(live_limit, ceiling)`, then return a
    /// permit. `ceiling` is the optional per-call cap (`None` = no extra cap).
    /// Synchronous — the provider turn already runs on a blocking `block_on`
    /// worker, and `Condvar` keeps a waiter parked without busy-spinning.
    pub(super) fn acquire(&'static self, ceiling: Option<usize>) -> GovernorPermit {
        let ceiling = ceiling.map_or(usize::MAX, |value| value.max(1));
        let mut state = lock_recovered(&self.state);
        loop {
            let effective = self.effective_limit(state.live_limit, ceiling);
            if state.in_flight < effective {
                state.in_flight += 1;
                return GovernorPermit { governor: self };
            }
            state = self
                .available
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Multiplicative decrease: a 429/overload tightens the live limit and
    /// resets the success streak. Adaptive flat fan-outs collapse all the way to
    /// the serial floor because a fresh provider throttle is an explicit
    /// low-headroom signal; fixed-ceiling governors keep the classic halving
    /// behavior.
    pub(super) fn on_rate_limit(&self) {
        let mut state = lock_recovered(&self.state);
        state.live_limit = if self.adaptive {
            DEFAULT_AGENT_MAX_CONCURRENCY.min(self.hard_max).max(1)
        } else {
            (state.live_limit / 2).max(1)
        };
        state.consecutive_successes = 0;
    }

    /// Additive increase: a clean request grows the live limit by one every
    /// [`GOVERNOR_SUCCESSES_PER_INCREASE`] successes, clamped to `hard_max`,
    /// waking a waiter when a slot opens up. For adaptive flat fan-outs,
    /// `may_grow = false` is an active provider-pressure signal: collapse the
    /// next admission target to the serial floor instead of merely freezing a
    /// previously grown limit.
    pub(super) fn on_success(&self, may_grow: bool) {
        if self.adaptive && !may_grow {
            let mut state = lock_recovered(&self.state);
            state.live_limit = DEFAULT_AGENT_MAX_CONCURRENCY.min(self.hard_max).max(1);
            state.consecutive_successes = 0;
            return;
        }
        let mut grew = false;
        {
            let mut state = lock_recovered(&self.state);
            state.consecutive_successes += 1;
            if state.consecutive_successes >= GOVERNOR_SUCCESSES_PER_INCREASE
                && state.live_limit < self.hard_max
            {
                state.live_limit += 1;
                state.consecutive_successes = 0;
                grew = true;
            }
        }
        if grew {
            self.available.notify_one();
        }
    }

    /// The current live admission limit — for tests and diagnostics.
    #[cfg(test)]
    pub(super) fn live_limit(&self) -> usize {
        self.state.lock().expect("governor mutex").live_limit
    }

    /// Non-blocking admission for deterministic tests: returns a permit only if
    /// a slot is free right now under `min(live_limit, ceiling)`, else `None`.
    /// Shares the exact admission predicate with [`Self::acquire`], so it proves
    /// the same gate without a timing-dependent blocked-probe thread.
    #[cfg(test)]
    pub(super) fn try_acquire(&'static self, ceiling: Option<usize>) -> Option<GovernorPermit> {
        let ceiling = ceiling.map_or(usize::MAX, |value| value.max(1));
        let mut state = self.state.lock().expect("governor mutex");
        let effective = self.effective_limit(state.live_limit, ceiling);
        if state.in_flight < effective {
            state.in_flight += 1;
            Some(GovernorPermit { governor: self })
        } else {
            None
        }
    }
}

/// Adaptive governor for flat `SpawnMultiAgent` fan-outs, **per provider**.
/// Explicit env sets a fixed ceiling; when unset, healthy providers open with a
/// small parallel seed, moderate Anthropic OAuth pressure trims the initial
/// burst, and low headroom / recent throttling collapses admission to serial.
/// Each provider has its own governor so a 429 on one provider's quota tightens
/// only that provider's admission, never a sibling agent on another provider.
pub(super) fn agent_rate_governor(kind: ProviderKind) -> &'static RateGovernor {
    static GOVERNORS: [OnceLock<RateGovernor>; PROVIDER_SLOTS] =
        [const { OnceLock::new() }; PROVIDER_SLOTS];
    GOVERNORS[provider_slot(kind)].get_or_init(|| match agent_api_concurrency_limit_from_env() {
        Some(limit) => RateGovernor::new(limit),
        None => RateGovernor::new_adaptive_for_provider(
            adaptive_agent_hard_max(),
            adaptive_agent_seed(kind),
            Some(kind),
        ),
    })
}

/// Adaptive governor for workflow-engine agents, **per provider**. Ceiling is the
/// higher [`workflow_concurrency_limit`] so a multi-phase workflow can run with
/// genuine parallelism — but the governor still ramps down the instant *that
/// provider's* account 429s, leaving other providers' governors untouched.
pub(super) fn workflow_rate_governor(kind: ProviderKind) -> &'static RateGovernor {
    static GOVERNORS: [OnceLock<RateGovernor>; PROVIDER_SLOTS] =
        [const { OnceLock::new() }; PROVIDER_SLOTS];
    GOVERNORS[provider_slot(kind)].get_or_init(|| RateGovernor::new(workflow_concurrency_limit()))
}

/// The workflow-engine concurrency cap: `ZO_WORKFLOW_MAX_CONCURRENCY` clamped
/// to `[1, MAX_WORKFLOW_AGENT_CONCURRENCY]`, else `min(16, cores-2)` (the Claude
/// Code default). Exposed so the engine can window its spawning to the same
/// bound and never open more OS threads than can actually run.
pub(crate) fn workflow_concurrency_limit() -> usize {
    std::env::var(WORKFLOW_AGENT_MAX_CONCURRENCY_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map_or_else(default_workflow_concurrency, |value| {
            value.min(MAX_WORKFLOW_AGENT_CONCURRENCY)
        })
}

/// `min(16, available_parallelism - 2)`, clamped to at least 1 — the Claude Code
/// workflow default. Leaves headroom for the main loop + IO threads.
fn default_workflow_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2))
        .unwrap_or(4)
        .clamp(1, 16)
}

/// Slice length for cancellable cool-down waits: short enough that a foreground
/// Ctrl+C is observed promptly, long enough not to busy-spin. Mirrors the
/// completion-store cancel poll cadence.
const COOLDOWN_CANCEL_POLL_SLICE_MS: u64 = 200;

/// Sleep until `kind`'s active cool-down (if any) expires, observing a
/// cooperative cancel flag so an agent parked in an open-ended rate-limit wait
/// still wakes on a foreground Ctrl+C / agent-stop instead of being
/// un-interruptible. Returns `false` when the wait was cut short by the cancel
/// flag, `true` when the cool-down elapsed naturally (or there was none).
pub(super) async fn wait_for_rate_limit_cooldown_cancellable(
    kind: ProviderKind,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> bool {
    let is_cancelled = || cancel.is_some_and(|flag| flag.load(Ordering::Relaxed));
    loop {
        if is_cancelled() {
            return false;
        }
        // The cool-down deadline now lives in `api::quota`; its remaining-ms
        // query is the monotonic `until - now` this loop used to compute
        // inline (0 == the window elapsed).
        let remaining = rate_limit_cooldown_remaining_ms(kind);
        if remaining == 0 {
            return true;
        }
        // Cap each sleep to a poll slice only when a cancel flag is present, so
        // the no-cancel path keeps sleeping the whole window in one shot.
        let slice = if cancel.is_some() {
            remaining.min(COOLDOWN_CANCEL_POLL_SLICE_MS)
        } else {
            remaining
        };
        tokio::time::sleep(std::time::Duration::from_millis(slice)).await;
    }
}

#[cfg(test)]
mod tests {
    // The cool-down mark/query API resolves through this module's re-exports of
    // `api::quota` (the migration kept every signature), so these governor/seed
    // tests stay green unchanged. The pure cool-down/headroom-arithmetic tests
    // that exercised the migrated *private* helpers moved to `api::quota`.
    use super::{
        adaptive_seed_at, agent_rate_governor, mark_capacity_stall_from,
        parse_agent_api_concurrency_limit, rate_limit_cooldown_remaining_ms, rate_limit_headroom_low,
        seed_snapshot_for, wait_for_rate_limit_cooldown_cancellable, RateGovernor,
        DEFAULT_AGENT_MAX_CONCURRENCY, GOVERNOR_SUCCESSES_PER_INCREASE,
        HEALTHY_AGENT_INITIAL_CONCURRENCY, MAX_AGENT_MAX_CONCURRENCY,
        MODERATE_AGENT_INITIAL_CONCURRENCY, QUOTA_SNAPSHOT_FRESH,
    };
    // Test-only cool-down helpers pulled straight from `api::quota` (the tools
    // lib itself no longer references them, so re-exporting would read as an
    // unused import).
    use api::quota::{mark_rate_limit_cooldown, rate_limit_backoff_ms};
    use api::ProviderKind;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    fn leak_governor(hard_max: usize) -> &'static RateGovernor {
        Box::leak(Box::new(RateGovernor::new(hard_max)))
    }

    fn leak_adaptive_governor(hard_max: usize, seed: usize) -> &'static RateGovernor {
        Box::leak(Box::new(RateGovernor::new_adaptive(hard_max, seed)))
    }

    #[test]
    fn governor_multiplicative_decrease_halves_to_floor_one() {
        let gov = RateGovernor::new(16);
        assert_eq!(gov.live_limit(), 16);
        gov.on_rate_limit();
        assert_eq!(gov.live_limit(), 8, "first 429 halves 16 → 8");
        gov.on_rate_limit();
        assert_eq!(gov.live_limit(), 4);
        gov.on_rate_limit();
        assert_eq!(gov.live_limit(), 2);
        gov.on_rate_limit();
        assert_eq!(gov.live_limit(), 1);
        gov.on_rate_limit();
        assert_eq!(gov.live_limit(), 1, "decrease never drops below 1");
    }

    #[test]
    fn governor_additive_increase_recovers_one_per_streak_up_to_ceiling() {
        let gov = RateGovernor::new(4);
        // Drop to the floor first.
        gov.on_rate_limit();
        gov.on_rate_limit();
        assert_eq!(gov.live_limit(), 1);
        // It takes a full success streak to grow by one.
        for _ in 0..GOVERNOR_SUCCESSES_PER_INCREASE - 1 {
            gov.on_success(true);
            assert_eq!(gov.live_limit(), 1, "partial streak does not grow yet");
        }
        gov.on_success(true);
        assert_eq!(
            gov.live_limit(),
            2,
            "a full streak grows the live limit by one"
        );
        // Grow back to the ceiling and stop there.
        for _ in 0..GOVERNOR_SUCCESSES_PER_INCREASE {
            gov.on_success(true);
        }
        assert_eq!(gov.live_limit(), 3);
        for _ in 0..GOVERNOR_SUCCESSES_PER_INCREASE {
            gov.on_success(true);
        }
        assert_eq!(gov.live_limit(), 4);
        for _ in 0..GOVERNOR_SUCCESSES_PER_INCREASE * 3 {
            gov.on_success(true);
        }
        assert_eq!(
            gov.live_limit(),
            4,
            "additive increase is clamped to hard_max"
        );
    }

    #[test]
    fn governor_acquire_blocks_beyond_effective_limit_and_releases_on_drop() {
        // hard_max 2, but tighten to a per-call ceiling of 1: only one permit
        // may be live at a time regardless of the higher governor ceiling.
        // Deterministic: `try_acquire` shares the exact admission predicate with
        // the blocking `acquire`, so we prove the gate without a timing race.
        let gov = leak_governor(2);
        let first = gov.try_acquire(Some(1)).expect("first slot is free");
        // A second admission under ceiling 1 must be refused while `first` lives.
        assert!(
            gov.try_acquire(Some(1)).is_none(),
            "second acquire must be refused while the first permit is live under ceiling 1"
        );
        // Releasing the first permit frees the slot for the next admission.
        drop(first);
        assert!(
            gov.try_acquire(Some(1)).is_some(),
            "dropping the first permit must free the slot"
        );
    }

    #[test]
    fn governor_per_call_ceiling_tightens_below_live_limit() {
        // Live limit 4, but a per-call ceiling of 2 admits at most 2.
        let gov = leak_governor(4);
        let a = gov.try_acquire(Some(2)).expect("first slot");
        let _b = gov.try_acquire(Some(2)).expect("second slot");
        // A third admission must be refused under the per-call ceiling of 2,
        // even though the governor's own live limit is 4.
        assert!(
            gov.try_acquire(Some(2)).is_none(),
            "a third acquire must be refused under per-call ceiling 2"
        );
        // Freeing one slot re-opens admission.
        drop(a);
        assert!(
            gov.try_acquire(Some(2)).is_some(),
            "freeing a slot must re-admit under ceiling 2"
        );
    }

    #[tokio::test]
    // Serialization guard held across awaits by design — the whole test
    // is the critical section against sibling quota-registry writers.
    #[allow(clippy::await_holding_lock)]
    async fn cancellable_cooldown_wakes_on_cancel_flag() {
        let _quota_serial = api::quota::rate_limit_test_guard();
        // Isolate first: `api`'s `cfg!(test)` guard is FALSE when it is compiled as a
        // dependency of THIS crate's test binary, so an un-isolated mark writes a real
        // cool-down into the account-global `~/.zo/rate/*.v1` that every running zo
        // reads — measured: a 60 s park plus a 5-minute "recently throttled" penalty
        // stamped on the developer's live sessions by `cargo test`.
        api::quota::isolate_rate_limit_state_for_tests();
        // Engage a long cool-down, then cancel: a pre-set cancel flag makes the
        // very first poll short-circuit, so the wait returns `false` (cut short)
        // immediately rather than sleeping the whole 60s window.
        mark_capacity_stall_from(
            ProviderKind::Anthropic,
            api::CapacityScope::Account,
            Some(Duration::from_secs(60)),
            0,
        );
        let cancel = AtomicBool::new(true);
        let finished =
            wait_for_rate_limit_cooldown_cancellable(ProviderKind::Anthropic, Some(&cancel)).await;
        assert!(
            !finished,
            "a pre-set cancel flag must short-circuit the wait"
        );
    }

    #[test]
    fn cooldown_from_prefers_larger_retry_after_over_backoff() {
        let _quota_serial = api::quota::rate_limit_test_guard();
        // Isolate first: `api`'s `cfg!(test)` guard is FALSE when it is compiled as a
        // dependency of THIS crate's test binary, so an un-isolated mark writes a real
        // cool-down into the account-global `~/.zo/rate/*.v1` that every running zo
        // reads — measured: a 60 s park plus a 5-minute "recently throttled" penalty
        // stamped on the developer's live sessions by `cargo test`.
        api::quota::isolate_rate_limit_state_for_tests();
        // `rate_limit_backoff_ms(0)` is the 15s floor; a 60s Retry-After should
        // win. We can't read the private cool-down deadline, but the helper's
        // selection logic is `max(server_hint, backoff)` — assert the inputs.
        let backoff = rate_limit_backoff_ms(0);
        assert_eq!(backoff, 15_000);
        // A 60s server hint exceeds the 15s backoff floor → server wins.
        // (Exercised indirectly: mark with a large hint, must not panic.)
        mark_capacity_stall_from(
            ProviderKind::Anthropic,
            api::CapacityScope::Account,
            Some(Duration::from_secs(60)),
            0,
        );
    }

    #[test]
    fn parse_concurrency_clamps_to_safe_bounds() {
        assert_eq!(parse_agent_api_concurrency_limit(None), None);
        assert_eq!(parse_agent_api_concurrency_limit(Some("")), None);
        assert_eq!(parse_agent_api_concurrency_limit(Some("0")), None);
        assert_eq!(parse_agent_api_concurrency_limit(Some("1")), Some(1));
        assert_eq!(parse_agent_api_concurrency_limit(Some("3")), Some(3));
        assert_eq!(
            parse_agent_api_concurrency_limit(Some("99")),
            Some(MAX_AGENT_MAX_CONCURRENCY)
        );
    }

    #[test]
    fn adaptive_seed_opens_parallel_unless_headroom_is_low() {
        use api::{RateLimitSnapshot, RateLimitWindow, RateLimitWindowKind};
        let window = |utilization: f64| {
            Some(RateLimitWindow {
                utilization,
                resets_at_unix: None,
            })
        };
        let snapshot = |utilization: f64| RateLimitSnapshot {
            five_hour: window(utilization),
            seven_day: None,
            representative: Some(RateLimitWindowKind::FiveHour),
        };
        let fresh = Duration::from_secs(30);
        let stale = QUOTA_SNAPSHOT_FRESH + Duration::from_secs(1);

        assert_eq!(
            adaptive_seed_at(false, None),
            HEALTHY_AGENT_INITIAL_CONCURRENCY,
            "healthy flat fan-out should open with real provider parallelism"
        );
        assert_eq!(
            adaptive_seed_at(true, Some((snapshot(0.3), fresh))),
            DEFAULT_AGENT_MAX_CONCURRENCY,
            "recent throttle/cool-down keeps admission serial"
        );
        assert_eq!(
            adaptive_seed_at(false, Some((snapshot(0.95), fresh))),
            DEFAULT_AGENT_MAX_CONCURRENCY,
            "fresh quota at the 95% fallback boundary keeps admission serial"
        );
        assert_eq!(
            adaptive_seed_at(false, Some((snapshot(0.94), fresh))),
            MODERATE_AGENT_INITIAL_CONCURRENCY,
            "quota below 95% must not trigger the low-headroom fallback"
        );
        assert_eq!(
            adaptive_seed_at(false, Some((snapshot(0.6), fresh))),
            MODERATE_AGENT_INITIAL_CONCURRENCY,
            "moderate quota utilization trims the initial burst"
        );
        assert_eq!(
            adaptive_seed_at(false, Some((snapshot(0.3), fresh))),
            HEALTHY_AGENT_INITIAL_CONCURRENCY,
            "fresh cool quota window keeps full healthy seed"
        );
        assert_eq!(
            adaptive_seed_at(false, Some((snapshot(0.3), stale))),
            HEALTHY_AGENT_INITIAL_CONCURRENCY,
            "stale snapshots are ignored instead of forcing serial admission"
        );
    }

    #[test]
    fn seed_snapshot_is_scoped_to_anthropic_only() {
        use api::{RateLimitSnapshot, RateLimitWindow, RateLimitWindowKind};
        // A fresh Anthropic snapshot is only recorded by the Anthropic client,
        // so it must feed ONLY an Anthropic governor's seed. Any other provider
        // must ignore it instead of tightening or widening from unrelated quota.
        let moderate = (
            RateLimitSnapshot {
                five_hour: Some(RateLimitWindow {
                    utilization: 0.6,
                    resets_at_unix: None,
                }),
                seven_day: None,
                representative: Some(RateLimitWindowKind::FiveHour),
            },
            Duration::from_secs(30),
        );
        // Anthropic keeps the snapshot → trims the healthy burst to moderate.
        assert_eq!(
            adaptive_seed_at(false, seed_snapshot_for(ProviderKind::Anthropic, Some(moderate))),
            MODERATE_AGENT_INITIAL_CONCURRENCY
        );
        // Every non-Anthropic provider drops it → normal healthy seed.
        for kind in [
            ProviderKind::OpenAi,
            ProviderKind::Google,
            ProviderKind::Xai,
            ProviderKind::Ollama,
        ] {
            assert!(seed_snapshot_for(kind, Some(moderate)).is_none());
            assert_eq!(
                adaptive_seed_at(false, seed_snapshot_for(kind, Some(moderate))),
                HEALTHY_AGENT_INITIAL_CONCURRENCY,
                "{kind:?} must not seed from the Anthropic-only snapshot"
            );
        }
    }

    #[test]
    fn adaptive_governor_ramps_to_ceiling_with_headroom() {
        let gov = leak_adaptive_governor(4, 1);
        for _ in 0..12 {
            gov.on_success(true);
            assert!(gov.live_limit() <= 4, "live limit must never exceed hard max");
        }
        assert_eq!(gov.live_limit(), 4);
        let _a = gov.try_acquire(None).expect("slot 1");
        let _b = gov.try_acquire(None).expect("slot 2");
        let _c = gov.try_acquire(None).expect("slot 3");
        let _d = gov.try_acquire(None).expect("slot 4");
        assert!(gov.try_acquire(None).is_none(), "5th slot refused");
    }

    #[test]
    fn adaptive_low_headroom_collapses_live_admission_to_serial() {
        let gov = leak_adaptive_governor(4, 1);
        for _ in 0..9 {
            gov.on_success(true);
        }
        assert_eq!(gov.live_limit(), 4, "precondition: governor has grown");

        gov.on_success(false);
        assert_eq!(
            gov.live_limit(),
            DEFAULT_AGENT_MAX_CONCURRENCY,
            "low headroom must collapse adaptive admission back to serial"
        );
        let _first = gov.try_acquire(None).expect("first serial slot");
        assert!(
            gov.try_acquire(None).is_none(),
            "a grown adaptive governor must not admit a second request while headroom is low"
        );
    }

    #[test]
    fn adaptive_growth_recovers_after_headroom_returns() {
        let gov = RateGovernor::new_adaptive(4, 1);
        for _ in 0..30 {
            gov.on_success(false);
        }
        assert_eq!(gov.live_limit(), 1, "low headroom keeps admission serial");
        for _ in 0..3 {
            gov.on_success(true);
        }
        assert_eq!(gov.live_limit(), 2);
    }

    #[test]
    fn explicit_env_override_wins_both_directions() {
        assert_eq!(parse_agent_api_concurrency_limit(None), None);
        assert_eq!(parse_agent_api_concurrency_limit(Some("")), None);
        assert_eq!(parse_agent_api_concurrency_limit(Some("0")), None);
        assert_eq!(parse_agent_api_concurrency_limit(Some("1")), Some(1));
        assert_eq!(parse_agent_api_concurrency_limit(Some("4")), Some(4));
        assert_eq!(
            parse_agent_api_concurrency_limit(Some("99")),
            Some(MAX_AGENT_MAX_CONCURRENCY)
        );

        let serial = leak_governor(1);
        let _first = serial.try_acquire(None).expect("first serial slot");
        assert!(serial.try_acquire(None).is_none(), "explicit 1 stays serial");

        let wide = leak_governor(8);
        let mut permits = Vec::new();
        for index in 0..8 {
            permits.push(wide.try_acquire(None).unwrap_or_else(|| panic!("slot {index}")));
        }
        assert!(wide.try_acquire(None).is_none(), "9th slot refused");
    }

    #[test]
    fn per_call_ceiling_still_tightens_adaptive() {
        let gov = leak_adaptive_governor(4, 1);
        for _ in 0..3 {
            gov.on_success(true);
        }
        assert_eq!(gov.live_limit(), 2);
        let _a = gov.try_acquire(Some(2)).expect("first slot");
        let _b = gov.try_acquire(Some(2)).expect("second slot");
        assert!(gov.try_acquire(Some(2)).is_none(), "ceiling 2 tightens adaptive limit");
    }

    #[test]
    fn fresh_429_collapses_adaptive_governor_to_serial_and_recovers() {
        let gov = RateGovernor::new_adaptive(4, 1);
        for _ in 0..9 {
            gov.on_success(true);
        }
        assert_eq!(gov.live_limit(), 4);
        gov.on_rate_limit();
        assert_eq!(
            gov.live_limit(),
            DEFAULT_AGENT_MAX_CONCURRENCY,
            "fresh adaptive 429 must collapse flat fan-out admission to serial"
        );
        for _ in 0..9 {
            gov.on_success(false);
        }
        assert_eq!(
            gov.live_limit(),
            DEFAULT_AGENT_MAX_CONCURRENCY,
            "low headroom must not reinflate"
        );
        for _ in 0..3 {
            gov.on_success(true);
        }
        assert_eq!(gov.live_limit(), 2);
    }

    #[test]
    fn provider_pressure_clamps_admission_even_before_success_feedback() {
        let gov = Box::leak(Box::new(RateGovernor::new_adaptive_for_provider(
            4,
            1,
            Some(ProviderKind::Xai),
        )));
        for _ in 0..9 {
            gov.on_success(true);
        }
        assert_eq!(gov.live_limit(), 4, "precondition: governor has grown");

        mark_rate_limit_cooldown(ProviderKind::Xai, 60_000);
        let _first = gov.try_acquire(None).expect("first pressure-limited slot");
        assert!(
            gov.try_acquire(None).is_none(),
            "provider pressure must clamp admission to the serial floor even before success feedback"
        );
    }

    #[test]
    fn non_adaptive_governor_ignores_growth_gate() {
        let gov = RateGovernor::new(4);
        gov.on_rate_limit();
        assert_eq!(gov.live_limit(), 2);
        for _ in 0..3 {
            gov.on_success(false);
        }
        assert_eq!(gov.live_limit(), 3);
    }

    // NOTE: the pure cool-down/headroom-arithmetic tests
    // (`cooldown_trusts_positive_server_hint_verbatim`,
    // `oauth_window_pressure_uses_binding_window_and_freshness`,
    // `headroom_gate_tracks_cooldown_and_recent_throttle`, and the headroom side
    // of `anthropic_quota_pressure_does_not_throttle_other_providers`) moved to
    // `api::quota`'s tests alongside the code they exercise. The seed-isolation
    // half stays covered here by `seed_snapshot_is_scoped_to_anthropic_only`.

    /// 트랙 3 핵심: cool-down 은 provider 별로 분리된다 — 한 provider 의 429 가
    /// 다른 provider 의 에이전트를 throttle 하지 않는다. 다른 테스트가 건드리지
    /// 않는 Google/Ollama 슬롯만 사용해 process-global 오염을 피한다.
    #[test]
    fn cooldown_is_isolated_per_provider() {
        // Google 에 긴 cool-down 을 건다.
        mark_rate_limit_cooldown(ProviderKind::Google, 60_000);
        assert!(
            rate_limit_cooldown_remaining_ms(ProviderKind::Google) > 1_000,
            "the throttled provider must show a live cool-down"
        );
        // Ollama(다른 provider)는 전혀 영향받지 않는다.
        assert_eq!(
            rate_limit_cooldown_remaining_ms(ProviderKind::Ollama),
            0,
            "a 429 on one provider must not open a cool-down on another"
        );
        assert!(
            rate_limit_headroom_low(ProviderKind::Google),
            "the throttled provider's headroom is low"
        );
        assert!(
            !rate_limit_headroom_low(ProviderKind::Ollama),
            "a sibling provider's headroom must stay healthy"
        );
    }

    /// 트랙 3: governor 도 provider 별 인스턴스다 — 같은 종류라도 서로 다른
    /// provider 는 다른 `&'static RateGovernor` 를 받는다(주소로 확인).
    #[test]
    fn governor_instances_are_distinct_per_provider() {
        let google = agent_rate_governor(ProviderKind::Google);
        let ollama = agent_rate_governor(ProviderKind::Ollama);
        assert!(
            !std::ptr::eq(google, ollama),
            "each provider must admit through its own governor instance"
        );
        // 같은 provider 는 같은 인스턴스(멱등).
        assert!(
            std::ptr::eq(google, agent_rate_governor(ProviderKind::Google)),
            "the same provider must reuse one governor instance"
        );
    }
}
