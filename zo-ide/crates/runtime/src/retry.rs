//! Retry with jittered exponential backoff for transient API errors.
//!
//! Claude Code CLI retries transient HTTP errors (429, 500, 502, 503, 529)
//! with exponential backoff. This module provides the same capability for
//! the Rust runtime, giving it parity and — thanks to lower per-retry
//! overhead — an edge in recovery speed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use core_types::retry_signal::{classify_error_text, reset_hint_in_text, RetrySignal};

/// Maximum number of retry attempts (excluding the initial attempt).
const MAX_RETRIES: u32 = 4;

/// Polling slice for [`cancellable_sleep`]. Short enough that a cooperative
/// cancel during a multi-second backoff is observed promptly, long enough to
/// stay effectively free when no cancel arrives.
pub(crate) const CANCEL_POLL_SLICE: Duration = Duration::from_millis(100);

/// Base delay for the first retry (doubles each subsequent attempt).
const BASE_DELAY: Duration = Duration::from_millis(500);

/// Base delay for rate-limit (429) errors — longer to respect limits.
const RATE_LIMIT_BASE_DELAY: Duration = Duration::from_secs(5);

/// Maximum per-retry delay cap.
const MAX_DELAY: Duration = Duration::from_secs(30);

/// Wall-clock budget for riding out a rate-limit / overload (429 / 529 /
/// "overloaded") window before giving up. A subscription throttle or a provider
/// overload routinely lasts *minutes*, so the old tiny attempt budget (≈65 s)
/// gave up mid-window and returned `Fail` — which kills the turn and forces the
/// user to manually re-issue "continue" into a still-throttled limit. With a
/// wall-clock budget we keep retrying (on the 30 s-capped rate-limit backoff)
/// until capacity frees up and the turn resumes on its own. Long enough to
/// outlast a per-minute throttle, short enough that a hard daily cap doesn't
/// hang forever — and the caller's cancel flag (Ctrl+C / esc) aborts the wait at
/// any point regardless.
const RATE_LIMIT_MAX_ELAPSED: Duration = Duration::from_secs(300);

/// Wall-clock budget for riding out a **provider overload** (529 /
/// `overloaded_error`), as opposed to this account's window.
///
/// A small fraction of [`RATE_LIMIT_MAX_ELAPSED`], because the two walls clear
/// differently and reward opposite reactions. Measured on the reported turn:
/// twelve whole-request attempts across the full 300 s account budget, all shed,
/// while a Haiku sub-agent on the *same account* succeeded in the same second —
/// the account was 2 % utilized. Re-sending the shed request shape is what does
/// not work; a lighter one is what does.
///
/// Sized against what happens *when it expires*, which is the part that changed:
/// the escape is no longer "swap providers or die" but a one-tier demotion on the
/// same provider (`QuotaEscape::Lighter`), announced and reverted next turn. Once
/// the fallback is productive, extra insistence on the chosen model buys nothing
/// but a longer freeze — a live run measured 109 s of spinner before a demotion
/// that then succeeded on the first try. This budget still spends ~12 real
/// attempts on the user's model first (each L2 attempt is 1 + up to 5 transparent
/// stream re-opens), which is ample patience for a blip.
const OVERLOAD_MAX_ELAPSED: Duration = Duration::from_secs(30);

/// How many *capacity* retries the MAIN turn spends before the error propagates
/// so the caller's quota escape can run (`main_turn_rate_limit_retry_cap`).
///
/// Two retries on the rate-limit schedule, ≈5 s then ≈10 s, which absorbs the
/// short burst that clears by itself and then hands over. The wall-clock budget
/// above is the right patience only when riding it out is the *only* recovery;
/// with an escape installed it is pure freeze, because the escape is consulted
/// only once this call returns `Fail`. Measured on the reported turn (t-5499): a
/// `gemini-flash` account 429 with no reset hint spent ten backoffs and 240 s of
/// the [`RATE_LIMIT_MAX_ELAPSED`] budget before the swap ran, while the same
/// prompt answered in 4.6 s on another model — the same reasoning
/// [`OVERLOAD_MAX_ELAPSED`] already records, that insistence buys nothing once
/// the fallback is productive.
pub(crate) const MAIN_TURN_CAPACITY_BURST: u32 = 2;

/// Env override (milliseconds) for the capacity wall-clock budgets. `0` opts out
/// of wall-clock mode entirely and falls back to the bounded [`MAX_RETRIES`]
/// attempt count (the pre-wall-clock behaviour); a bad value uses the defaults.
///
/// A set value replaces the account budget verbatim and clamps the overload
/// budget to it, so one knob can only ever *shorten* the wait — there is no way
/// to configure "wait longer on an overload than on a throttle", which would be
/// backwards.
const RATE_LIMIT_MAX_ELAPSED_ENV: &str = "ZO_RATE_LIMIT_MAX_WAIT_MS";

/// Resolve the wall-clock budget for `signal`. `None` means "no wall-clock
/// budget" (env opt-out), in which case capacity errors fall back to the bounded
/// attempt schedule.
fn capacity_max_elapsed(signal: RetrySignal) -> Option<Duration> {
    let default = if signal.is_overloaded() {
        OVERLOAD_MAX_ELAPSED
    } else {
        RATE_LIMIT_MAX_ELAPSED
    };
    match std::env::var(RATE_LIMIT_MAX_ELAPSED_ENV) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(ms) => Some(Duration::from_millis(ms).min(default)),
            Err(_) => Some(default),
        },
        Err(_) => Some(default),
    }
}

/// Transient error classifier result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryVerdict {
    /// The error is transient — retry after the recommended delay.
    Retry { delay: Duration },
    /// The error is permanent or retries are exhausted — propagate.
    Fail,
}

/// Determine whether an error message represents a transient failure
/// that should be retried.
///
/// The classifier inspects the stringified error for known patterns:
/// - HTTP 429 (rate limit / overloaded)
/// - HTTP 500, 502, 503, 529 (server errors)
/// - Connection/timeout errors
///
/// `attempt` is 0-indexed (0 = first attempt just failed); `elapsed` is the
/// wall-clock time already spent retrying this operation (used only for the
/// rate-limit budget — pass `Duration::ZERO` for a pure attempt-based check).
#[must_use]
pub fn classify_for_retry(error_message: &str, attempt: u32, elapsed: Duration) -> RetryVerdict {
    // Provider clients already spent their own retry budget before returning
    // this wrapper. Re-entering the whole provider ladder here turns one retry
    // into another six HTTP attempts and defeats verifier failover.
    let normalized = error_message.to_ascii_lowercase();
    if normalized.contains("api failed after ") && normalized.contains(" attempts:") {
        return RetryVerdict::Fail;
    }

    // The retry *vocabulary* (which words mean "rate limit" vs "transient" vs
    // "fatal") lives in one place — `core_types::retry_signal` — so the api
    // layer, the conversation UI, and the stream parser all classify from the
    // same words. This site owns only the *consequence*.
    let signal = classify_error_text(error_message);
    if !signal.is_retryable() {
        return RetryVerdict::Fail;
    }

    // A capacity stall (429 / 529 / overloaded) is bounded by a *wall-clock*
    // budget, not the tiny attempt count: these windows last minutes, so giving
    // up after ~65 s returns `Fail` → the turn dies and the user must manually
    // re-issue "continue" into a still-throttled limit. Riding it out on the
    // 30 s-capped rate-limit backoff lets the turn resume by itself the moment
    // capacity frees up. The caller's cancel flag still aborts the wait.
    //
    // The budget depends on WHOSE capacity ran out — see [`OVERLOAD_MAX_ELAPSED`]:
    // an account window is worth waiting minutes for, a provider shedding load is
    // not (the escape hatch is a different model, and the caller runs it as soon
    // as this returns `Fail`).
    if signal.is_capacity() {
        match capacity_max_elapsed(signal) {
            Some(budget) if elapsed < budget => {
                // The provider said when the wall lifts. A reset the budget
                // cannot reach (a plan window hours away) is handed over at
                // once — riding out five minutes of retries against it was
                // the reported behaviour — and a reachable one is waited for
                // in one sleep instead of a ladder of blind ones.
                let remaining = budget.saturating_sub(elapsed);
                let hint = reset_hint_in_text(error_message);
                if hint.is_some_and(|hint| hint > remaining) {
                    return RetryVerdict::Fail;
                }
                return RetryVerdict::Retry {
                    delay: rate_limit_backoff(attempt).max(hint.unwrap_or_default()),
                };
            }
            Some(_) => return RetryVerdict::Fail,
            // Env opt-out: fall through to the bounded attempt schedule below.
            None => {}
        }
    }

    // Generic transient blips (and capacity stalls when the wall-clock budget is
    // disabled) use the bounded attempt schedule.
    if attempt >= MAX_RETRIES {
        return RetryVerdict::Fail;
    }
    let delay = if signal.is_capacity() {
        rate_limit_backoff(attempt)
    } else {
        backoff_delay(attempt)
    };
    RetryVerdict::Retry { delay }
}

/// Feed a foreground main-turn capacity stall into the process-global
/// per-provider cool-down state (`api::quota`) so the router's headroom penalty
/// and the sub-agent admission gate observe the SAME throttle the main turn
/// just hit — closing the gap where only sub-agent 429s were recorded (the
/// foreground `retry_async` path here previously rode out the window without
/// ever marking the shared state).
///
/// Fires only for capacity signals — classified from the same
/// `core_types::retry_signal` vocabulary this module's backoff classifier uses,
/// so a generic 5xx / timeout blip (which does not consume provider quota) is
/// ignored — and routes each one by **scope**:
///
/// * an account 429 climbs the exponential cool-down ladder and stamps this
///   account as throttled, exactly as before;
/// * a provider overload takes a short, flat pause and stamps *nothing about
///   the account*. Charging it to the account ladder is what wrote a 120 s
///   cross-process park (plus a 5-minute headroom penalty and a lagging
///   "~10 % remaining" estimate) onto a window measured at 2 % utilization.
///
/// `attempt` is 0-indexed (the just-failed attempt) and drives the account
/// ladder only. No structured `Retry-After` is available at this `Display`-only
/// error seam, so `None` is passed; the shared helper still honors a present
/// server hint wherever one IS available (the sub-agent path).
pub fn mark_foreground_capacity_stall(model: &str, error_message: &str, attempt: u32) {
    let Some(scope) = capacity_scope(classify_error_text(error_message)) else {
        return;
    };
    api::quota::mark_capacity_stall_from(
        api::detect_provider_kind(model),
        scope,
        None,
        attempt,
    );
}

/// Map a flattened-text [`RetrySignal`] onto the structured
/// [`api::CapacityScope`], or `None` when the signal is not capacity pressure.
///
/// The two enums are deliberately separate — one is the text fallback, the other
/// travels with a structured error — and this is the single place they are joined
/// so they cannot drift into disagreeing about which wall was hit.
#[must_use]
pub fn capacity_scope(signal: RetrySignal) -> Option<api::CapacityScope> {
    match signal {
        RetrySignal::RateLimit => Some(api::CapacityScope::Account),
        RetrySignal::Overloaded => Some(api::CapacityScope::Provider),
        RetrySignal::Transient | RetrySignal::Fatal => None,
    }
}

/// Longer backoff for rate-limit (429) errors.
fn rate_limit_backoff(attempt: u32) -> Duration {
    backoff_delay_from(attempt, RATE_LIMIT_BASE_DELAY)
}

/// Compute the backoff delay for the given attempt with jitter.
///
/// Uses exponential backoff with ±25% jitter to prevent thundering herd.
fn backoff_delay(attempt: u32) -> Duration {
    backoff_delay_from(attempt, BASE_DELAY)
}

/// Shared exponential-backoff core for both the standard and rate-limit
/// schedules. They differ only in `base` — the exponential growth, the
/// [`MAX_DELAY`] cap, and the ±25% deterministic jitter are identical, so the
/// single implementation lives here and the two named entry points select the
/// base. Keeping one body means the cap/jitter logic can never drift between
/// the two schedules.
///
/// The jitter is a cheap hash of the attempt number (no RNG) — it just spreads
/// retry storms so concurrent clients don't re-fire in lockstep.
fn backoff_delay_from(attempt: u32, base: Duration) -> Duration {
    let base_ms = u64::try_from(base.as_millis()).unwrap_or(u64::MAX);
    // `attempt` can grow unbounded while riding out a rate-limit window on the
    // wall-clock budget, so cap the shift exponent — the result is `MAX_DELAY`-
    // capped just below anyway, and `1u64 << 64+` would overflow/panic.
    let exponential_ms = base_ms.saturating_mul(1u64 << attempt.min(32));
    let max_delay_ms = u64::try_from(MAX_DELAY.as_millis()).unwrap_or(u64::MAX);
    let capped_ms = exponential_ms.min(max_delay_ms);

    let jitter_range = capped_ms / 4;
    let jitter_offset = u64::from(attempt).wrapping_mul(2_654_435_761) % (jitter_range * 2 + 1);
    let jittered_ms = capped_ms
        .saturating_sub(jitter_range)
        .saturating_add(jitter_offset);

    Duration::from_millis(jittered_ms)
}

/// Sleep for `delay`, but wake early if `cancel` flips to `true`. Returns
/// `true` when it was cancelled mid-sleep, `false` when the full delay elapsed.
///
/// Polls a shared `AtomicBool` in short slices instead of taking a new
/// dependency on a cancellation primitive — the codebase's cooperative-cancel
/// convention. With `cancel == None` it is a plain `sleep`.
async fn cancellable_sleep(delay: Duration, cancel: Option<&AtomicBool>) -> bool {
    let Some(cancel) = cancel else {
        tokio::time::sleep(delay).await;
        return false;
    };
    let mut remaining = delay;
    while !remaining.is_zero() {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        let slice = remaining.min(CANCEL_POLL_SLICE);
        tokio::time::sleep(slice).await;
        remaining = remaining.saturating_sub(slice);
    }
    cancel.load(Ordering::Relaxed)
}

/// Async retry helper that retries a future-producing closure on transient
/// errors.
///
/// `operation_name` is used for logging. `make_future` is called with the
/// current attempt number (0-indexed). `on_error(attempt, error)` fires for
/// every failed call before classification, including terminal/fail-fast
/// errors, so state such as provider cool-downs is not coupled to whether a
/// retry is scheduled. `on_retry(next_attempt, delay, error)` fires once per
/// scheduled retry *before* the backoff sleep — `next_attempt` is the upcoming
/// 1-based retry number — so a live UI can surface "retrying in Ns" instead of
/// going silent for the whole wait. `cancel`, when set, aborts the backoff sleep
/// and stops retrying — the last error propagates — so a foreground Ctrl+C is
/// observed during the wait instead of only at retry boundaries.
/// `rate_limit_retry_cap` bounds how many *capacity* retries this call may spend
/// while leaving generic transient retries untouched: `None` rides out the whole
/// `RATE_LIMIT_MAX_ELAPSED` budget, `Some(0)` returns the first capacity error
/// so a higher-level cross-provider failover can run immediately, and `Some(n)`
/// absorbs a short burst before handing over.
pub async fn retry_async<F, Fut, T, E, O, R>(
    _operation_name: &str,
    cancel: Option<&AtomicBool>,
    rate_limit_retry_cap: Option<u32>,
    mut on_error: O,
    mut on_retry: R,
    mut make_future: F,
) -> Result<T, E>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
    O: FnMut(u32, &E),
    R: FnMut(u32, Duration, &E),
{
    let started = Instant::now();
    let mut attempt = 0u32;
    let mut rate_limit_retries = 0u32;
    loop {
        match make_future(attempt).await {
            Ok(value) => return Ok(value),
            Err(error) => {
                on_error(attempt, &error);
                let error_text = error.to_string();
                // The cap counts *capacity* retries of either scope: its purpose
                // is "hand over to a higher-level failover instead of absorbing
                // the wall here", and a provider overload has a failover to hand
                // to just as much as a 429 does.
                let is_capacity = classify_error_text(&error_text).is_capacity();
                let capped =
                    is_capacity && rate_limit_retry_cap.is_some_and(|cap| rate_limit_retries >= cap);
                let verdict = if capped {
                    RetryVerdict::Fail
                } else {
                    classify_for_retry(&error_text, attempt, started.elapsed())
                };
                match verdict {
                    RetryVerdict::Retry { delay } => {
                        if is_capacity {
                            rate_limit_retries += 1;
                        }
                        on_retry(attempt + 1, delay, &error);
                        if cancellable_sleep(delay, cancel).await {
                            return Err(error);
                        }
                        attempt += 1;
                    }
                    RetryVerdict::Fail => return Err(error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_429_as_transient() {
        let verdict = classify_for_retry("HTTP 429 Too Many Requests", 0, Duration::ZERO);
        assert!(matches!(verdict, RetryVerdict::Retry { .. }));
    }

    #[test]
    fn classifies_500_as_transient() {
        let verdict = classify_for_retry("HTTP 500 Internal Server Error", 0, Duration::ZERO);
        assert!(matches!(verdict, RetryVerdict::Retry { .. }));
    }

    #[test]
    fn classifies_520_unknown_status_as_transient() {
        let verdict = classify_for_retry(
            "api returned 520 <unknown status code>: error code: 520",
            0,
            Duration::ZERO,
        );
        assert!(matches!(verdict, RetryVerdict::Retry { .. }));
    }

    #[test]
    fn classifies_overloaded_as_transient() {
        let verdict = classify_for_retry("overloaded_error: Overloaded", 0, Duration::ZERO);
        assert!(matches!(verdict, RetryVerdict::Retry { .. }));
    }

    #[test]
    fn classifies_timeout_as_transient() {
        let verdict = classify_for_retry("request timed out", 0, Duration::ZERO);
        assert!(matches!(verdict, RetryVerdict::Retry { .. }));
    }

    #[test]
    fn classifies_rate_limit_as_transient() {
        let verdict =
            classify_for_retry("rate_limit_error: rate limit exceeded", 0, Duration::ZERO);
        assert!(matches!(verdict, RetryVerdict::Retry { .. }));
    }

    #[test]
    fn classifies_backend_terminal_stream_failure_as_transient() {
        let verdict = classify_for_retry(
            "turn: runtime: provider stream: transport error: api stream error: backend reported a terminal stream failure",
            0,
            Duration::ZERO,
        );
        assert!(matches!(verdict, RetryVerdict::Retry { .. }));
    }

    /// A capacity refusal that names its reset is not ridden out blindly: a
    /// reset past the budget hands over at once (the quota escape runs on the
    /// very first refusal), and a reachable one becomes the single delay.
    #[test]
    fn a_named_reset_is_honored_not_ridden_out() {
        let usage_limit = "runtime: provider stream: transport error: api returned 429 Too Many \
                           Requests (usage_limit_reached): The usage limit has been reached; \
                           retry-after: 8220";
        assert_eq!(
            classify_for_retry(usage_limit, 0, Duration::ZERO),
            RetryVerdict::Fail,
            "a window hours away is past every budget"
        );
        let soon = "api returned 429 Too Many Requests (rate_limit_error): slow down; retry-after: 40";
        assert_eq!(
            classify_for_retry(soon, 0, Duration::ZERO),
            RetryVerdict::Retry {
                delay: Duration::from_secs(40)
            },
            "a reachable reset is waited for once"
        );
        assert_eq!(
            classify_for_retry(soon, 0, RATE_LIMIT_MAX_ELAPSED.saturating_sub(Duration::from_secs(10))),
            RetryVerdict::Fail,
            "a reset past what is left of the budget hands over"
        );
    }

    /// The refusal the provider client raises *before sending*, when the
    /// shared registry already says the window is parked, must read here
    /// exactly like a 429 that came back over the wire: the seconds it names
    /// survive the flattening, so a park past what is left of the budget hands
    /// the turn over at once instead of sleeping against it.
    #[test]
    fn a_park_refused_before_sending_reads_like_a_wire_rate_limit() {
        let parked =
            api::ApiError::rate_limited_before_send("anthropic", Duration::from_secs(60))
                .to_string();
        assert_eq!(
            classify_for_retry(&parked, 0, Duration::ZERO),
            RetryVerdict::Retry {
                delay: Duration::from_secs(60)
            },
            "a reachable park is waited out once: {parked}"
        );
        assert_eq!(
            classify_for_retry(
                &parked,
                0,
                RATE_LIMIT_MAX_ELAPSED.saturating_sub(Duration::from_secs(10))
            ),
            RetryVerdict::Fail,
            "a park past what is left of the budget hands over: {parked}"
        );
    }

    #[test]
    fn exhausted_provider_retry_ladder_is_terminal() {
        let verdict = classify_for_retry(
            "api failed after 6 attempts: api returned 429 Too Many Requests",
            0,
            Duration::ZERO,
        );
        assert_eq!(
            verdict,
            RetryVerdict::Fail,
            "the runtime must not re-enter a provider retry ladder that already exhausted"
        );
    }

    #[test]
    fn classifies_auth_error_as_permanent() {
        let verdict =
            classify_for_retry("authentication_error: invalid API key", 0, Duration::ZERO);
        assert_eq!(verdict, RetryVerdict::Fail);
    }

    #[test]
    fn classifies_validation_error_as_permanent() {
        let verdict =
            classify_for_retry("invalid_request_error: messages too long", 0, Duration::ZERO);
        assert_eq!(verdict, RetryVerdict::Fail);
    }

    #[test]
    fn exhausts_transient_retries_after_max() {
        // A generic transient (non-capacity) blip is bounded by the attempt count.
        let verdict =
            classify_for_retry("HTTP 500 Internal Server Error", MAX_RETRIES, Duration::ZERO);
        assert_eq!(verdict, RetryVerdict::Fail);
    }

    #[test]
    fn rate_limit_retries_past_attempt_budget_within_wall_clock() {
        // A 429 / overload must NOT give up at MAX_RETRIES: it rides the window
        // out on the wall-clock budget so the turn resumes instead of dying and
        // forcing a manual "continue" into a still-throttled limit.
        let verdict = classify_for_retry(
            "HTTP 429 Too Many Requests",
            MAX_RETRIES + 10,
            Duration::from_secs(10),
        );
        assert!(
            matches!(verdict, RetryVerdict::Retry { .. }),
            "rate-limit must keep retrying past the attempt budget while inside the wall-clock window"
        );
    }

    #[test]
    fn rate_limit_gives_up_after_wall_clock_budget() {
        let verdict = classify_for_retry(
            "HTTP 429 Too Many Requests",
            2,
            RATE_LIMIT_MAX_ELAPSED + Duration::from_secs(1),
        );
        assert_eq!(
            verdict,
            RetryVerdict::Fail,
            "rate-limit must give up once the wall-clock budget is spent"
        );
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn backoff_increases_exponentially() {
        let d0 = backoff_delay(0);
        let d1 = backoff_delay(1);
        let d2 = backoff_delay(2);
        // Each delay should be roughly double the previous (within jitter)
        assert!(d1.as_millis() > d0.as_millis());
        assert!(d2.as_millis() > d1.as_millis());
        // But never exceeds the cap
        assert!(d2 <= MAX_DELAY + Duration::from_millis(MAX_DELAY.as_millis() as u64 / 4));
    }

    #[test]
    fn backoff_is_capped() {
        let d10 = backoff_delay(10);
        // Even at attempt 10, delay should be near MAX_DELAY (not overflow)
        assert!(d10.as_millis() <= (MAX_DELAY.as_millis() * 5 / 4));
    }

    #[tokio::test]
    async fn cancellable_sleep_wakes_early_on_cancel() {
        let cancel = AtomicBool::new(true);
        let start = std::time::Instant::now();
        let was_cancelled = cancellable_sleep(Duration::from_secs(30), Some(&cancel)).await;
        assert!(was_cancelled);
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "a pre-set cancel must not wait out the full delay"
        );
    }

    #[tokio::test]
    async fn cancellable_sleep_completes_without_cancel() {
        let start = std::time::Instant::now();
        let was_cancelled = cancellable_sleep(Duration::from_millis(50), None).await;
        assert!(!was_cancelled);
        assert!(start.elapsed() >= Duration::from_millis(40));
    }

    #[tokio::test]
    async fn retry_sleep_aborts_on_cancel() {
        // A transient error schedules a backoff; with the cancel flag set, the
        // sleep is abandoned and the last error propagates instead of retrying.
        let cancel = AtomicBool::new(true);
        let attempts = std::cell::Cell::new(0u32);
        let start = std::time::Instant::now();
        let result: Result<(), String> = retry_async(
            "test",
            Some(&cancel),
            None,
            |_, _| {},
            |_, _, _| {},
            |attempt| {
                attempts.set(attempt + 1);
                async move { Err::<(), String>("HTTP 429 Too Many Requests".to_string()) }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1, "cancel must stop further retry attempts");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "the rate-limit backoff must be cut short by cancel"
        );
    }

    #[test]
    fn fail_fast_rate_limit_marks_cooldown_without_runtime_retry() {
        // This observer mutates the same process-wide quota state as the
        // foreground test. Hold its shared guard outside the async runtime,
        // and keep test marks away from the person's account-global file.
        let _quota_serial = api::quota::rate_limit_test_guard();
        api::quota::isolate_rate_limit_state_for_tests();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let model = "claude-sonnet-4-5";
        let kind = api::detect_provider_kind(model);
        let attempts = std::cell::Cell::new(0u32);
        let observed_errors = std::cell::Cell::new(0u32);
        let notices = std::cell::Cell::new(0u32);
        let result: Result<(), String> = runtime.block_on(retry_async(
            "test",
            None,
            Some(0),
            |attempt, error: &String| {
                observed_errors.set(observed_errors.get() + 1);
                mark_foreground_capacity_stall(model, error, attempt);
            },
            |_, _, _| notices.set(notices.get() + 1),
            |attempt| {
                attempts.set(attempt + 1);
                async move { Err::<(), String>("HTTP 429 Too Many Requests".to_string()) }
            },
        ));
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1, "fail-fast must return the first 429");
        assert_eq!(
            observed_errors.get(),
            1,
            "the terminal 429 must reach the cooldown observer"
        );
        assert!(
            api::quota::rate_limit_cooldown_remaining_ms(kind) > 0,
            "the fail-fast 429 must open the provider cooldown"
        );
        assert_eq!(notices.get(), 0, "fail-fast must not schedule a backoff");
    }

    #[tokio::test]
    async fn rate_limit_cap_of_one_absorbs_a_burst_then_hands_over() {
        // The VERIFY leg's no-cross-candidate policy: one short capacity retry
        // (so a burst 429 does not degrade verification onto the model that just
        // wrote the diff) and then the error propagates so the ranked walk can
        // fall back — instead of the full wall-clock ladder (ten backoffs).
        // Sleeps one real backoff: `rate_limit_backoff(0)` is a deterministic
        // 3.75s (jitter offset is 0 at attempt 0), and the crate's tokio dev
        // features do not include `test-util`, so there is no virtual clock.
        let attempts = std::cell::Cell::new(0u32);
        let notices = std::cell::Cell::new(0u32);
        let result: Result<(), String> = retry_async(
            "test",
            None,
            Some(1),
            |_, _| {},
            |_, _, _| notices.set(notices.get() + 1),
            |attempt| {
                attempts.set(attempt + 1);
                async move { Err::<(), String>("HTTP 429 Too Many Requests".to_string()) }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.get(), 2, "the cap must allow exactly one retry");
        assert_eq!(notices.get(), 1, "exactly one backoff is announced");
    }

    /// The main turn's burst cap: with an escape installed the stream absorbs
    /// [`MAIN_TURN_CAPACITY_BURST`] capacity retries and then propagates, so
    /// `decide_quota_escape` runs in seconds instead of after the full
    /// [`RATE_LIMIT_MAX_ELAPSED`] account budget (t-5499: ten backoffs, 240 s,
    /// in front of a fallback that answered the same prompt in 4.6 s).
    ///
    /// Sleeps two real rate-limit backoffs — the crate's tokio dev features do
    /// not include `test-util`, so there is no virtual clock — which is why the
    /// elapsed assertion is a ceiling well under the budget rather than an
    /// equality.
    #[tokio::test]
    async fn main_turn_capacity_burst_hands_over_on_the_third_capacity_error() {
        let attempts = std::cell::Cell::new(0u32);
        let notices = std::cell::Cell::new(0u32);
        let started = Instant::now();
        let result: Result<(), String> = retry_async(
            "test",
            None,
            Some(MAIN_TURN_CAPACITY_BURST),
            |_, _| {},
            |_, _, _| notices.set(notices.get() + 1),
            |attempt| {
                attempts.set(attempt + 1);
                async move {
                    Err::<(), String>(
                        "api returned 429 Too Many Requests (RESOURCE_EXHAUSTED)".to_string(),
                    )
                }
            },
        )
        .await;
        let elapsed = started.elapsed();
        eprintln!("[t-5499] main-turn burst handed over after {} ms", elapsed.as_millis());
        assert!(result.is_err(), "the third capacity error must propagate");
        assert_eq!(
            attempts.get(),
            MAIN_TURN_CAPACITY_BURST + 1,
            "two retries are absorbed, the third capacity error hands over"
        );
        assert_eq!(
            notices.get(),
            MAIN_TURN_CAPACITY_BURST,
            "one backoff notice per absorbed retry"
        );
        assert!(
            elapsed <= Duration::from_secs(20),
            "handing over took {elapsed:?}; the point of the cap is seconds, not the \
             {RATE_LIMIT_MAX_ELAPSED:?} account budget"
        );
    }

    #[tokio::test]
    async fn rate_limit_cap_leaves_generic_transient_retries_intact() {
        // The cap is capacity-only: a 5xx blip still rides the ordinary bounded
        // attempt schedule even under the strictest (`Some(0)`) cap, so verifier
        // failover never turns a transport hiccup into an instant failure.
        let cancel = AtomicBool::new(false);
        let notices = std::cell::Cell::new(0u32);
        let _: Result<(), String> = retry_async(
            "test",
            Some(&cancel),
            Some(0),
            |_, _| {},
            |_, _, _| {
                notices.set(notices.get() + 1);
                // Stop after proving the first retry was scheduled.
                cancel.store(true, Ordering::Relaxed);
            },
            |_attempt| async move { Err::<(), String>("HTTP 500 Internal Server Error".to_string()) },
        )
        .await;
        assert_eq!(
            notices.get(),
            1,
            "a generic 5xx must still schedule a retry under a capacity cap"
        );
    }

    #[test]
    fn overloaded_uses_the_longer_rate_limit_backoff() {
        // An `overloaded_error` (HTTP 529) is a capacity signal: it must back off
        // on the rate-limit schedule (seconds), not the short transient one
        // (sub-second), so the provider pool has time to recover.
        let RetryVerdict::Retry { delay: overload } = classify_for_retry(
            "transport error: api stream error (overloaded_error): Overloaded",
            0,
            Duration::ZERO,
        ) else {
            panic!("overloaded must be retried");
        };
        let RetryVerdict::Retry { delay: server } =
            classify_for_retry("HTTP 500 Internal Server Error", 0, Duration::ZERO)
        else {
            panic!("500 must be retried");
        };
        assert!(
            overload >= RATE_LIMIT_BASE_DELAY.saturating_sub(RATE_LIMIT_BASE_DELAY / 4),
            "overload backoff ({overload:?}) must use the rate-limit schedule"
        );
        assert!(
            overload > server,
            "overload ({overload:?}) must wait longer than a generic 5xx ({server:?})"
        );
    }

    #[tokio::test]
    async fn on_retry_fires_before_each_backoff() {
        // The notice hook lets a live UI report the stall; assert it is invoked
        // once per scheduled retry with the 1-based attempt number and a delay.
        let cancel = AtomicBool::new(true); // abort after the first notice
        let notices = std::cell::RefCell::new(Vec::new());
        let _: Result<(), String> = retry_async(
            "test",
            Some(&cancel),
            None,
            |_, _| {},
            |attempt, delay, error: &String| {
                notices.borrow_mut().push((attempt, delay, error.clone()));
            },
            |_attempt| async move { Err::<(), String>("overloaded_error: Overloaded".to_string()) },
        )
        .await;
        let notices = notices.into_inner();
        assert_eq!(
            notices.len(),
            1,
            "one notice before the (cancelled) backoff"
        );
        assert_eq!(notices[0].0, 1, "first retry is reported as attempt 1");
        assert!(
            notices[0].1 > Duration::ZERO,
            "a positive backoff is reported"
        );
    }

    /// The foreground main-turn 429 must feed the SAME per-provider cool-down
    /// state (`api::quota`) the sub-agent path reads — and only for genuine
    /// rate-limit/overload signals. We read the provider `detect_provider_kind`
    /// resolves for a fixed model and assert the transition on that exact slot
    /// (robust to whichever kind the env maps it to); no other runtime test
    /// marks the cool-down state, so the slot is clean.
    #[test]
    fn foreground_rate_limit_marks_provider_cooldown() {
        let _quota_serial = api::quota::rate_limit_test_guard();
        // Isolate first: `api`'s `cfg!(test)` guard is FALSE when it is compiled as a
        // dependency of THIS crate's test binary, so an un-isolated mark writes a real
        // cool-down into the account-global `~/.zo/rate/*.v1` that every running zo
        // reads — measured: a 60 s park plus a 5-minute "recently throttled" penalty
        // stamped on the developer's live sessions by `cargo test`.
        api::quota::isolate_rate_limit_state_for_tests();
        let model = "claude-sonnet-4-5";
        let kind = api::detect_provider_kind(model);
        // A generic 5xx is not a capacity signal → it must NOT open a cool-down.
        let before = api::quota::rate_limit_cooldown_remaining_ms(kind);
        mark_foreground_capacity_stall(model, "HTTP 500 Internal Server Error", 0);
        assert_eq!(
            api::quota::rate_limit_cooldown_remaining_ms(kind),
            before,
            "a 5xx blip must not mark a rate-limit cool-down"
        );
        // A 429 is a capacity stall → it must open the provider's cool-down.
        mark_foreground_capacity_stall(model, "HTTP 429 Too Many Requests", 0);
        assert!(
            api::quota::rate_limit_cooldown_remaining_ms(kind) > 0,
            "a foreground 429 must open the provider cool-down the router/spawn gate reads"
        );
    }

    /// A flattened error string maps to the same scope the structured class
    /// carries, so the text-only seams (this module) and the class-carrying seams
    /// (`decide_quota_escape`) can never disagree about which wall was hit.
    #[test]
    fn capacity_scope_matches_the_structured_class() {
        assert_eq!(
            capacity_scope(classify_error_text("HTTP 429 Too Many Requests")),
            Some(api::CapacityScope::Account)
        );
        assert_eq!(
            capacity_scope(classify_error_text(
                "api stream error (overloaded_error): Overloaded"
            )),
            Some(api::CapacityScope::Provider)
        );
        assert_eq!(
            capacity_scope(classify_error_text("HTTP 500 Internal Server Error")),
            None
        );
    }

    /// A provider overload gets a much shorter ride than an account throttle.
    ///
    /// The measured failure spent the full 300 s account budget on ~12 whole-request
    /// retries of a request shape the provider was shedding, then died. The overload
    /// budget exists so that time is spent on the escape (a lighter model) instead.
    #[test]
    fn overload_budget_is_shorter_than_the_account_budget() {
        let overload = "api stream error (overloaded_error): Overloaded";
        let throttle = "api returned 429 Too Many Requests (rate_limit_error)";
        let past_overload = OVERLOAD_MAX_ELAPSED + Duration::from_secs(1);

        // Inside its own budget an overload still retries — a genuine blip is
        // absorbed without disturbing the turn. Derived from the const, not a
        // literal, so retuning the budget cannot silently un-test this arm.
        assert!(matches!(
            classify_for_retry(overload, 3, OVERLOAD_MAX_ELAPSED / 2),
            RetryVerdict::Retry { .. }
        ));
        // Past it, the retry layer hands over instead of grinding.
        assert_eq!(
            classify_for_retry(overload, 3, past_overload),
            RetryVerdict::Fail
        );
        // The account throttle is unchanged at that same elapsed time: only time
        // lifts its window, so riding it out is still the best available move.
        assert!(matches!(
            classify_for_retry(throttle, 3, past_overload),
            RetryVerdict::Retry { .. }
        ));
        assert!(OVERLOAD_MAX_ELAPSED < RATE_LIMIT_MAX_ELAPSED);
    }
}
