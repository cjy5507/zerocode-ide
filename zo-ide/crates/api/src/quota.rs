//! Process-global quota state: the OAuth-subscription unified windows and the
//! per-provider 429/cool-down bookkeeping — the two quota signals every layer
//! reads without threading state through the call graph.
//!
//! Zo is OAuth-subscription-first: the unified 5h/7d windows Anthropic
//! returns on every response are the account's authoritative quota signal, and
//! unlike a 429 (a *lagging* indicator — the account is already throttled) the
//! utilization is a *leading* one. Every Anthropic response — foreground turn
//! or sub-agent — writes the freshest snapshot here so quota-aware policies
//! (the sub-agent spawn headroom gate, starvation labels, the router headroom
//! penalty) can read it. Readers must treat staleness explicitly: the snapshot
//! carries its age, and an old high utilization may already have reset.
//!
//! The per-provider 429/cool-down state lives here too (moved out of the
//! `tools` crate so BOTH the foreground main-turn retry path — `runtime`, which
//! cannot depend on `tools` — AND the sub-agent admission path feed and read
//! ONE shared throttle record). The `tools` crate keeps only the AIMD
//! admission *governor* (concurrency is a sub-agent-domain concern) and
//! delegates its cool-down mark/query API here with unchanged signatures.
//! Partitioned per [`ProviderKind`] so a Gemini 429 never parks a Claude turn,
//! and `DeepSeek`/Grok fold onto their OpenAI/xAI slots (see [`provider_slot`]).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use core_types::{RateLimitSnapshot, RateLimitWindow, RateLimitWindowKind};

use crate::sync_bridge::lock_recovered;
use crate::{CapacityScope, ProviderKind};

/// Latest snapshot with the instant it was observed. Poison policy: recover —
/// the only write is a single `Copy` assignment, so the value is consistent
/// at every panic point.
static LATEST_RATE_LIMIT: Mutex<Option<(RateLimitSnapshot, Instant)>> = Mutex::new(None);

/// Record a freshly parsed unified snapshot. Empty snapshots (API-key
/// responses carry no unified headers) are ignored so they cannot mask a
/// recent subscription reading with a blank one.
pub fn record_rate_limit_snapshot(snapshot: RateLimitSnapshot) {
    if !snapshot.has_data() {
        return;
    }
    // Cross-process leading signal: publish the binding window's utilization so
    // every zo process on this account throttles as the *shared* window
    // fills, not just the one that made the request. Only Anthropic records
    // these OAuth snapshots, so key it to Anthropic. Throttled + no-op when the
    // coordination file is disabled (see `quota_shared`).
    if let Some((_, window)) = binding_window(snapshot) {
        crate::quota_shared::record_utilization(ProviderKind::Anthropic, window.utilization);
    }
    *lock_recovered(&LATEST_RATE_LIMIT) = Some((snapshot, Instant::now()));
}

/// One quota window a provider reports about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaWindow {
    /// Short window name as the provider frames it (`"5h"`, `"7d"`, `"weekly"`).
    pub label: String,
    /// Remaining headroom percent (`0..=100`), NOT utilization — the same
    /// direction [`ProviderQuotaView`] uses, converted once at the edge.
    pub remaining_percent: u8,
    /// Absolute clock time this window rolls over, when the provider says.
    pub resets_at_unix: Option<u64>,
    /// The model this allowance is scoped to, when the provider bills one
    /// separately from the account-wide window. `None` is account-wide.
    ///
    /// Both subscription endpoints report the two side by side: ChatGPT lists
    /// `additional_rate_limits[].limit_name`, Anthropic a `weekly_scoped` entry
    /// carrying `scope.model.display_name`. They are NOT a second opinion about
    /// the account window — a model can sit at 0% while the plan has room —
    /// which is why [`ProviderQuotaView`] carries the distinction rather than
    /// flattening both into one list.
    pub model: Option<String>,
}

/// A provider's quota as *it* reports it, rather than as we infer it.
///
/// The 429-derived rows in `estimated_quota_view` are a lagging guess: they
/// only exist once the account is already throttled, and they cannot say how
/// much headroom is left before that. Every subscription provider we speak to
/// exposes its own usage endpoint, and this is what one of those answers maps
/// to — so a freshly opened session knows its standing without having spent a
/// request to learn it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredQuota {
    /// Windows the provider reports, in the order it lists them. Empty means
    /// "the account has no limits worth showing", which is not the same as
    /// "unknown" — an unknown provider records nothing at all.
    pub windows: Vec<QuotaWindow>,
    /// Signed-in account for this provider, when its credential carries one.
    /// Held here because the credential read that yields the quota is the same
    /// one that identifies the account; splitting them would read it twice.
    pub account: Option<String>,
}

/// Latest measured quota per provider, with the instant it was observed.
///
/// Poison policy matches [`LATEST_RATE_LIMIT`]: recover. A write replaces one
/// slot wholesale, so the array is consistent at every panic point.
static MEASURED_QUOTA: Mutex<[Option<(MeasuredQuota, Instant)>; PROVIDER_SLOTS]> =
    Mutex::new([const { None }; PROVIDER_SLOTS]);

/// Age past which a measured reading stops being reported.
///
/// A quota window moves on the order of hours, so a reading minutes old is
/// still true. What this guards is the opposite failure: a poller that died
/// leaves its last answer sitting in the slot forever, and a stale "80%
/// remaining" is worse than no figure at all because it invites spending
/// against headroom that is gone.
const MEASURED_QUOTA_FRESH: Duration = Duration::from_secs(15 * 60);

/// Record a provider's own reading of its quota. See [`MeasuredQuota`].
pub fn record_measured_quota(kind: ProviderKind, quota: MeasuredQuota) {
    lock_recovered(&MEASURED_QUOTA)[provider_slot(kind)] = Some((quota, Instant::now()));
}

/// The most recent measured reading for `kind` and its age, if one was recorded
/// this process lifetime. Staleness is the caller's to judge — the age is
/// returned rather than applied, matching [`latest_rate_limit_snapshot`].
#[must_use]
pub fn latest_measured_quota(kind: ProviderKind) -> Option<(MeasuredQuota, Duration)> {
    lock_recovered(&MEASURED_QUOTA)[provider_slot(kind)]
        .as_ref()
        .map(|(quota, at)| (quota.clone(), at.elapsed()))
}

/// Signed-in account for `kind`, when a measured reading carried one.
///
/// Only the *active* model's provider is ever shown, so this answers one
/// provider at a time rather than handing out the whole set: three accounts on
/// screen is three chances to leak an address in a screenshot for two of them
/// that nobody is currently spending.
#[must_use]
pub fn provider_account(kind: ProviderKind) -> Option<String> {
    latest_measured_quota(kind).and_then(|(quota, _)| quota.account)
}

/// The most recent unified snapshot and its age, if any response carried one
/// this process lifetime.
#[must_use]
pub fn latest_rate_limit_snapshot() -> Option<(RateLimitSnapshot, Duration)> {
    lock_recovered(&LATEST_RATE_LIMIT)
        .as_ref()
        .map(|(snapshot, at)| (*snapshot, at.elapsed()))
}

/// Neutralize every ambient rate-limit signal for the current process.
///
/// Test support for crates that depend on `api`. The throttle state below is
/// process-global and `crate::quota_shared`'s file is account-global, so a
/// quota-sensitive unit test otherwise observes whatever a sibling test — or an
/// unrelated `zo` running on the same account — happened to record. That is not
/// hypothetical: it is what made the deep-gate ranked-verifier walk skip its
/// first candidate and fail on a clean checkout.
///
/// Deliberately **not** `#[cfg(test)]`. That attribute is false in this crate
/// whenever `api` is compiled as a dependency of another crate's test binary,
/// which is precisely where the leak happens, so a `cfg`-gated helper would be
/// invisible to the tests that need it.
/// Serializes every test that resets ([`isolate_rate_limit_state_for_tests`])
/// or DRIVES the process-global rate-limit registry (streams a 429/overload
/// through the retry ladder, whose `on_error` hook marks provider cool-downs
/// in the same statics). Isolation alone cannot protect a parallel schedule:
/// it resets the state once, and a sibling test then re-marks a cool-down
/// mid-test — the ranked verifier walk pre-flight-skips that provider and the
/// assertion fails for a reason unrelated to the code under test. Hold the
/// guard for the WHOLE test, first line, before isolating.
///
/// Not `#[cfg(test)]` for the same reason as
/// [`isolate_rate_limit_state_for_tests`]: the callers live in OTHER crates'
/// test binaries, where this crate's `cfg(test)` is false.
pub fn rate_limit_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn isolate_rate_limit_state_for_tests() {
    // One-way, so a concurrently running sibling test cannot re-enable the
    // shared file underneath a test that has already isolated itself.
    crate::quota_shared::force_disable_for_process();
    *lock_recovered(&LATEST_RATE_LIMIT) = None;
    for (((cooldown, cooldown_unix), last_seen), overload) in RATE_LIMIT_COOLDOWN_UNTIL_MS
        .iter()
        .zip(RATE_LIMIT_COOLDOWN_UNTIL_UNIX_MS.iter())
        .zip(LAST_RATE_LIMIT_AT_MS.iter())
        .zip(OVERLOAD_PAUSE_UNTIL_MS.iter())
    {
        cooldown.store(0, Ordering::SeqCst);
        cooldown_unix.store(0, Ordering::SeqCst);
        last_seen.store(0, Ordering::SeqCst);
        overload.store(0, Ordering::SeqCst);
    }
    for seen in &OWN_RATE_LIMIT_SEEN {
        seen.store(false, Ordering::SeqCst);
    }
    for exhausted in &PROVIDER_QUOTA_EXHAUSTED_UNTIL_MS {
        exhausted.store(0, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Per-provider 429 / cool-down state (moved from `tools::agent_tools::rate_limit`)
// ---------------------------------------------------------------------------

/// Number of distinct [`ProviderKind`] buckets the per-provider rate state is
/// partitioned into. Each provider (Anthropic, xAI, OpenAI, Google, Ollama)
/// gets its own cool-down window so one provider's 429 never throttles requests
/// to a different provider — the whole point of running Claude/GPT/Gemini
/// sub-agents in parallel. Keep in sync with [`provider_slot`].
pub const PROVIDER_SLOTS: usize = 5;

/// Stable array index for a provider's rate-limit bucket. Exhaustive over
/// [`ProviderKind`] so adding a variant is a compile error here (forcing
/// `PROVIDER_SLOTS` + the static arrays to grow with it), not a silent
/// out-of-bounds or a collision that re-shares quota across providers.
#[must_use]
pub const fn provider_slot(kind: ProviderKind) -> usize {
    match kind {
        ProviderKind::Anthropic => 0,
        ProviderKind::Xai => 1,
        ProviderKind::OpenAi => 2,
        ProviderKind::Google => 3,
        ProviderKind::Ollama => 4,
    }
}

/// Per-provider rate-limit cool-down windows. When a request's provider stream
/// surfaces a 429 / `rate_limit` error the caller records "no request to **this
/// provider** should be issued before this instant", expressed as milliseconds
/// on the process-start monotonic clock ([`now_monotonic_millis`]). Partitioned
/// by [`provider_slot`] so a Gemini 429 never parks a Claude request.
static RATE_LIMIT_COOLDOWN_UNTIL_MS: [AtomicU64; PROVIDER_SLOTS] =
    [const { AtomicU64::new(0) }; PROVIDER_SLOTS];

/// Wall-clock (unix ms) companion to [`RATE_LIMIT_COOLDOWN_UNTIL_MS`]. The
/// monotonic deadline above stays authoritative for waits/headroom (immune to
/// NTP steps / manual clock changes); this parallel unix deadline exists ONLY
/// so a display/HUD surface can convert the cool-down reset to an absolute
/// clock time ([`provider_quota_views`]'s `resets_at_unix`). Both are set from
/// the same instant in [`mark_rate_limit_cooldown`] and only ratchet forward,
/// so they stay consistent.
static RATE_LIMIT_COOLDOWN_UNTIL_UNIX_MS: [AtomicU64; PROVIDER_SLOTS] =
    [const { AtomicU64::new(0) }; PROVIDER_SLOTS];

/// Per-provider monotonic timestamp (ms) of the most recent 429. `0` = never
/// throttled. Feeds the spawn headroom gate (W9-4): a fan-out launched right
/// after a throttle burst on that provider should not re-open at the top tier.
static LAST_RATE_LIMIT_AT_MS: [AtomicU64; PROVIDER_SLOTS] =
    [const { AtomicU64::new(0) }; PROVIDER_SLOTS];

/// Per-provider "this process took an account 429 for it" flag.
///
/// [`LAST_RATE_LIMIT_AT_MS`] cannot answer that question on its own: it holds
/// a monotonic reading whose zero means both "never throttled" and "throttled
/// in this process's first millisecond", which is exactly when a fresh `zo`
/// asks (a test binary reproduced it on the first line of its first test).
/// This flag only ever goes true, so the answer never flickers.
static OWN_RATE_LIMIT_SEEN: [AtomicBool; PROVIDER_SLOTS] =
    [const { AtomicBool::new(false) }; PROVIDER_SLOTS];

/// Per-provider monotonic deadline (ms) for a **provider-overload** pause
/// (529 / `overloaded_error`), kept apart from the account cool-down above.
///
/// Both mean "hold requests to this provider for a moment", and
/// [`rate_limit_cooldown_remaining_ms`] unions them for exactly that question.
/// What must NOT be unioned is everything else the account arrays imply — how
/// much of the plan's quota is left ([`provider_quota_views`]), whether this
/// account was recently throttled ([`LAST_RATE_LIMIT_AT_MS`]), and the
/// cross-process account park. A 529 answers none of those: it fired here with
/// the account window at 2 % utilization, and folding it into the account state
/// wrote a 120 s park plus a five-minute "throttled" penalty onto a healthy plan.
/// Separate arrays make that mistake unrepresentable rather than merely
/// discouraged.
static OVERLOAD_PAUSE_UNTIL_MS: [AtomicU64; PROVIDER_SLOTS] =
    [const { AtomicU64::new(0) }; PROVIDER_SLOTS];

/// Per-provider monotonic deadline (ms) until which the provider's quota is
/// *known* exhausted, taken from a 429's own `Retry-After` without the
/// [`RATE_LIMIT_COOLDOWN_MAX_MS`] clamp. `0` = nothing known.
///
/// Separate from the cool-down above because the two answer different
/// questions. The cool-down bounds how long a *caller may be parked*, and its
/// two-minute ceiling is deliberate: a hostile hint must never stall a user
/// turn. But a provider that reports "resets in 3 hours" is still exhausted at
/// minute three, and a decision that merely *chooses between providers* — the
/// cross-model VERIFY pre-flight — is strictly better off knowing that.
/// Skipping costs nothing there, while attempting guarantees another 429.
///
/// Nothing ever waits on this value, so an inflated hint can only cause a skip,
/// never a stall. That is what makes dropping the clamp safe here.
static PROVIDER_QUOTA_EXHAUSTED_UNTIL_MS: [AtomicU64; PROVIDER_SLOTS] =
    [const { AtomicU64::new(0) }; PROVIDER_SLOTS];

/// Ceiling on [`PROVIDER_QUOTA_EXHAUSTED_UNTIL_MS`]: generous enough for the
/// hour- and day-scale windows subscription plans actually use, bounded so a
/// garbage hint cannot mute a provider for the whole process lifetime.
const PROVIDER_QUOTA_EXHAUSTED_MAX_MS: u64 = 6 * 60 * 60 * 1_000;

/// Initial cool-down window applied on the first 429 of a burst.
const RATE_LIMIT_COOLDOWN_INITIAL_MS: u64 = 15_000;
/// Hard ceiling on the cool-down window so a hostile provider hint
/// can't stall the caller indefinitely.
const RATE_LIMIT_COOLDOWN_MAX_MS: u64 = 120_000;
/// Ceiling on a **provider-overload** cool-down (529 / `overloaded_error`).
///
/// Much lower than [`RATE_LIMIT_COOLDOWN_MAX_MS`] because the two windows are
/// different animals: an account throttle lasts as long as the plan's window,
/// while capacity shedding clears in seconds. Letting an overload climb the
/// account ladder parked a *healthy* account — measured: `utilization = 2 %`
/// with a 120 s park written to the shared file — which then depressed
/// sub-agent admission and router headroom for every concurrent process.
const OVERLOAD_COOLDOWN_MAX_MS: u64 = 15_000;
/// How long after the last 429 the process still counts as "throttled" for
/// the spawn headroom gate. 5 minutes: long enough to cover the minute-bucket
/// token-rate windows that caused the 2026-06-10 starvation incident, short
/// enough that a one-off 429 doesn't depress fan-out quality for the session.
const RATE_LIMIT_HEADROOM_WINDOW_MS: u64 = 300_000;

/// Utilization at/above which an Anthropic binding window may escape to another
/// model. Below 95%, a 429 is treated as transient burst pressure instead.
pub const QUOTA_FALLBACK_UTIL_THRESHOLD: f64 = 0.95;

/// Utilization at/above which the binding OAuth window counts as headroom-low
/// for spawning. Kept aligned with [`QUOTA_FALLBACK_UTIL_THRESHOLD`] so leading
/// admission pressure and model-swap policy agree on the 95% boundary.
pub const HEADROOM_UTIL_THRESHOLD: f64 = QUOTA_FALLBACK_UTIL_THRESHOLD;

/// Maximum snapshot age the leading-signal gates trust. Unified windows move
/// slowly (5h/7d), but a stale high reading may already have reset — 10 minutes
/// keeps the signal honest while surviving quiet gaps between requests.
pub const QUOTA_SNAPSHOT_FRESH: Duration = Duration::from_secs(600);

fn quota_fallback_permitted_at(
    kind: ProviderKind,
    snapshot: Option<(RateLimitSnapshot, Duration)>,
    shared_utilization: Option<(u64, Duration)>,
) -> bool {
    if !matches!(kind, ProviderKind::Anthropic) {
        return true;
    }

    let local = snapshot.and_then(|(snapshot, age)| {
        (age <= QUOTA_SNAPSHOT_FRESH)
            .then(|| binding_window(snapshot).map(|(_, window)| window.utilization))
            .flatten()
    });
    let shared = shared_utilization
        .filter(|(_, age)| *age <= QUOTA_SNAPSHOT_FRESH)
        .map(|(utilization_x1000, _)| utilization_x1000);

    local.is_some_and(|utilization| utilization >= QUOTA_FALLBACK_UTIL_THRESHOLD)
        || shared.is_some_and(|utilization| {
            utilization >= crate::quota_shared::SHARED_UTIL_THRESHOLD_X1000
        })
        || (local.is_none() && shared.is_none())
}

/// Whether a hard provider rate limit may swap to another model.
///
/// Anthropic swaps only when a fresh local or cross-process binding-window
/// reading is at least 95% utilized. A fresh cooler reading turns a 429 into a
/// same-model park/retry; when neither process-local nor shared utilization can
/// be measured freshly, fallback remains allowed so unknown state cannot park
/// a turn forever. Providers without measured quota windows retain the prior
/// fallback behavior.
#[must_use]
pub fn quota_fallback_permitted(kind: ProviderKind) -> bool {
    quota_fallback_permitted_at(
        kind,
        latest_rate_limit_snapshot(),
        crate::quota_shared::latest_utilization(kind),
    )
}

/// Fixed *remaining* estimate reported for a provider that 429'd recently but
/// whose active cool-down has already elapsed (a lagging "was throttled" hint,
/// not a measured figure — hence [`ProviderQuotaView::estimated`]). Low but
/// non-zero: the account was squeezed within the window, yet is not currently
/// parked.
const RECENT_RATE_LIMIT_REMAINING_PERCENT: u8 = 10;

/// Monotonic millisecond clock anchored at first use. Used instead of
/// `SystemTime` so the cool-down window can't be skewed by wall-clock jumps
/// (NTP steps, manual changes) that would make `until - now` underflow or
/// balloon — `Instant` is guaranteed non-decreasing.
fn now_monotonic_millis() -> u64 {
    static ANCHOR: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    u64::try_from(ANCHOR.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Wall-clock unix milliseconds (display-only companion to the monotonic
/// clock). Failures (a pre-epoch system clock) degrade to `0` rather than
/// panicking — a bogus reset time is a cosmetic HUD glitch, never a wait bug
/// (the monotonic deadline drives the actual waiting).
fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Wall-clock unix seconds — the granularity `RateLimitWindow::resets_at_unix`
/// uses, for the "this window already reset" freshness check in the views.
pub(crate) fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Exponential backoff for in-stream rate-limit retries: 15s, 30s, 60s,
/// 120s, 120s, ... (capped at `RATE_LIMIT_COOLDOWN_MAX_MS`). `attempt` is
/// 0-indexed (0 = first retry).
#[must_use]
pub fn rate_limit_backoff_ms(attempt: u32) -> u64 {
    let shift = attempt.min(3);
    let multiplier = 1u64 << shift;
    RATE_LIMIT_COOLDOWN_INITIAL_MS
        .saturating_mul(multiplier)
        .min(RATE_LIMIT_COOLDOWN_MAX_MS)
}

/// Extend the cool-down window for `kind` so it covers at least `extra_ms` from
/// now. Multiple concurrent failures only ratchet that provider's deadline
/// forward — they never shorten an existing window, and never touch a sibling
/// provider's window.
pub fn mark_rate_limit_cooldown(kind: ProviderKind, extra_ms: u64) {
    mark_cooldown(kind, extra_ms, CapacityScope::Account);
}

/// Shared core of the cool-down entry points. `scope` selects the ceiling and
/// which account-level state the mark is allowed to touch.
///
/// A [`CapacityScope::Provider`] mark still pauses this provider briefly — do not
/// hammer a backend that is shedding — but writes neither the "this account was
/// throttled" stamp (which pins a 5-minute headroom penalty and a lagging
/// `~10 % remaining` estimate onto a healthy window) nor the cross-process
/// account park. Both of those describe the *account's* window, and a 529 says
/// nothing about it: the measured utilization was 2 % while every request was
/// being shed.
fn mark_cooldown(kind: ProviderKind, extra_ms: u64, scope: CapacityScope) {
    let slot = provider_slot(kind);
    let now = now_monotonic_millis();
    let (deadlines, ceiling) = match scope {
        CapacityScope::Account => (&RATE_LIMIT_COOLDOWN_UNTIL_MS, RATE_LIMIT_COOLDOWN_MAX_MS),
        CapacityScope::Provider => (&OVERLOAD_PAUSE_UNTIL_MS, OVERLOAD_COOLDOWN_MAX_MS),
    };
    let capped = extra_ms.min(ceiling);
    if matches!(scope, CapacityScope::Account) {
        LAST_RATE_LIMIT_AT_MS[slot].store(now, Ordering::Relaxed);
        OWN_RATE_LIMIT_SEEN[slot].store(true, Ordering::Relaxed);
        // Wall-clock reset deadline for the display views — same instant/window as
        // the monotonic deadline below, ratcheted forward independently. Because
        // both clocks advance together, a mark that fails to advance the monotonic
        // deadline (a longer cool-down already active) also fails to advance this
        // one, so they never diverge.
        let unix_until = now_unix_millis().saturating_add(capped);
        RATE_LIMIT_COOLDOWN_UNTIL_UNIX_MS[slot].fetch_max(unix_until, Ordering::SeqCst);
        // Cross-process: park EVERY zo process on this shared account window, not
        // only the one that hit the 429. This is what stops five concurrent sessions
        // from each re-discovering the same wall. No-op for non-account-shared
        // providers and when coordination is disabled (see `quota_shared`).
        crate::quota_shared::record_cooldown(kind, unix_until);
    }
    let until = now.saturating_add(capped);
    let mut current = deadlines[slot].load(Ordering::Relaxed);
    loop {
        if until <= current {
            return;
        }
        match deadlines[slot].compare_exchange_weak(
            current,
            until,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

/// Engage `kind`'s cool-down from a structured capacity error, routed by
/// [`CapacityScope`].
///
/// One entry point for both walls so a caller cannot accidentally charge a
/// provider overload to the account's window — the mistake that wrote a 120 s
/// account park while the measured utilization was 2 %. `attempt` drives the
/// exponential ladder for an account throttle; an overload ignores it beyond
/// `OVERLOAD_COOLDOWN_MAX_MS`, since escalating a window that clears in
/// seconds only delays the recovery.
pub fn mark_capacity_stall_from(
    kind: ProviderKind,
    scope: CapacityScope,
    retry_after: Option<Duration>,
    attempt: u32,
) {
    match scope {
        CapacityScope::Account => {
            mark_cooldown(
                kind,
                cooldown_wait_ms(retry_after, attempt),
                CapacityScope::Account,
            );
            // Only an account 429 can report how long ITS window stays shut, so
            // only it may mute the provider for provider-choice decisions.
            mark_provider_quota_exhausted(kind, retry_after);
        }
        CapacityScope::Provider => mark_cooldown(
            kind,
            // A server hint still wins when present; the ladder never applies.
            retry_after
                .and_then(|hint| u64::try_from(hint.as_millis()).ok())
                .filter(|&ms| ms > 0)
                .unwrap_or(OVERLOAD_COOLDOWN_MAX_MS),
            CapacityScope::Provider,
        ),
    }
}

/// Record the unclamped horizon a 429 reported, for provider-choice decisions
/// only. No-op when the error carried no usable hint: an absent `Retry-After`
/// says nothing about the window's length, and guessing one from the retry
/// ladder would mute a provider over a transient burst.
fn mark_provider_quota_exhausted(kind: ProviderKind, retry_after: Option<Duration>) {
    let Some(hint_ms) = retry_after
        .and_then(|hint| u64::try_from(hint.as_millis()).ok())
        .filter(|&ms| ms > 0)
    else {
        return;
    };
    let until =
        now_monotonic_millis().saturating_add(hint_ms.min(PROVIDER_QUOTA_EXHAUSTED_MAX_MS));
    // Ratchet forward only, mirroring the cool-down: a stale shorter hint must
    // not shorten a window a later 429 already reported as longer.
    PROVIDER_QUOTA_EXHAUSTED_UNTIL_MS[provider_slot(kind)].fetch_max(until, Ordering::Relaxed);
}

/// Remaining window during which `kind`'s quota is known exhausted, `0` when
/// nothing is known or the window has passed.
///
/// For choosing *between* providers, never for waiting — see
/// `PROVIDER_QUOTA_EXHAUSTED_UNTIL_MS`.
#[must_use]
pub fn provider_quota_exhausted_remaining_ms(kind: ProviderKind) -> u64 {
    PROVIDER_QUOTA_EXHAUSTED_UNTIL_MS[provider_slot(kind)]
        .load(Ordering::Relaxed)
        .saturating_sub(now_monotonic_millis())
}

/// Pure core of [`mark_rate_limit_cooldown_from`]: a present, positive
/// `Retry-After` is authoritative and used verbatim (capped at
/// [`RATE_LIMIT_COOLDOWN_MAX_MS`]) — the official Anthropic SDKs do the same
/// ("if the API asks us to wait a reasonable amount, just do what it says").
/// The previous `max(hint, exponential)` floor over-waited small hints by up
/// to 8× once the ladder capped (server says 15s, ladder says 120s), which
/// prolonged exactly the starvation W9 fights. The exponential `attempt`
/// ladder applies only when the provider sent no usable hint.
fn cooldown_wait_ms(retry_after: Option<Duration>, attempt: u32) -> u64 {
    retry_after
        .map(|hint| u64::try_from(hint.as_millis()).unwrap_or(RATE_LIMIT_COOLDOWN_MAX_MS))
        .filter(|&hint_ms| hint_ms > 0)
        .map_or_else(
            || rate_limit_backoff_ms(attempt),
            |hint_ms| hint_ms.min(RATE_LIMIT_COOLDOWN_MAX_MS),
        )
}

/// Remaining cool-down window for `kind`, if one is active. Lets the provider
/// client stamp a truthful `rate-limited · resumes in ~Ns` phase on the agent
/// manifest before parking, instead of waiting invisibly, and drives the
/// cancellable cool-down wait in the `tools` crate.
#[must_use]
pub fn rate_limit_cooldown_remaining_ms(kind: ProviderKind) -> u64 {
    let slot = provider_slot(kind);
    let now = now_monotonic_millis();
    // "Hold requests to this provider" is the one question both windows answer,
    // so this is the one place they are unioned — an account throttle and a
    // provider overload park a caller just the same.
    let local = RATE_LIMIT_COOLDOWN_UNTIL_MS[slot]
        .load(Ordering::Relaxed)
        .max(OVERLOAD_PAUSE_UNTIL_MS[slot].load(Ordering::Relaxed))
        .saturating_sub(now);
    // Whichever window is longer wins: another process's shared 429 can park
    // this one even when its own monotonic window is clear.
    local.max(crate::quota_shared::cooldown_remaining_ms(kind))
}

/// Whether THIS process has taken an account 429 for `kind` since it started.
///
/// The question is not "how hot is the window?" — that is
/// [`rate_limit_headroom_low`] — but "is the park standing in the registry
/// knowledge this process earned, or knowledge it inherited?".
///
/// The provider clients' pre-send door needs that distinction. A park this
/// process earned is its own retry ladder's arithmetic (15 s, 30 s, 60 s …)
/// and the layers that wrote it — the client ladder, the runtime classifier
/// above it — already own when to try again. A park it did not earn came from
/// another `zo` on the same account, and nothing in this process knows about
/// it until a request comes back refused.
#[must_use]
pub fn took_own_rate_limit(kind: ProviderKind) -> bool {
    OWN_RATE_LIMIT_SEEN[provider_slot(kind)].load(Ordering::Relaxed)
}

/// Spawn headroom gate (W9-4): `true` while the process has little provider
/// headroom for `kind` — an active cool-down, a 429 within the last
/// `RATE_LIMIT_HEADROOM_WINDOW_MS`, or (for Anthropic only) the OAuth
/// subscription window running hot (`oauth_window_pressure_for`, the
/// *leading* signal). Read at sub-agent model selection so a fan-out
/// opened during/after a throttle burst starts one tier lower instead of
/// stampeding the throttled top tier.
#[must_use]
pub fn rate_limit_headroom_low(kind: ProviderKind) -> bool {
    let slot = provider_slot(kind);
    headroom_low_for_kind_at(
        kind,
        now_monotonic_millis(),
        // Admission is a "send now?" question, so an active overload pause counts
        // here (do not fan eight sub-agents into a shedding backend) — unlike the
        // quota *view*, which must not read a 529 as spent account quota. The
        // 5-minute post-throttle penalty still comes only from a real 429.
        RATE_LIMIT_COOLDOWN_UNTIL_MS[slot]
            .load(Ordering::Relaxed)
            .max(OVERLOAD_PAUSE_UNTIL_MS[slot].load(Ordering::Relaxed)),
        LAST_RATE_LIMIT_AT_MS[slot].load(Ordering::Relaxed),
        latest_rate_limit_snapshot(),
    )
    // A sibling process's shared cool-down / hot account window collapses this
    // process's admission to serial too, so the governors throttle together.
    || crate::quota_shared::headroom_low(kind)
}

fn headroom_low_for_kind_at(
    kind: ProviderKind,
    now_ms: u64,
    cooldown_until_ms: u64,
    last_rate_limit_at_ms: u64,
    snapshot: Option<(RateLimitSnapshot, Duration)>,
) -> bool {
    headroom_low_at(now_ms, cooldown_until_ms, last_rate_limit_at_ms)
        || oauth_window_pressure_for(kind, snapshot)
}

/// W9-4 leading signal: `true` when `kind`'s freshest unified quota snapshot
/// shows the binding window at/above [`HEADROOM_UTIL_THRESHOLD`]. Only Anthropic
/// records these OAuth subscription snapshots, so sibling OpenAI/Google/xAI/
/// Ollama providers never inherit Anthropic quota pressure.
fn oauth_window_pressure_for(
    kind: ProviderKind,
    snapshot: Option<(RateLimitSnapshot, Duration)>,
) -> bool {
    matches!(kind, ProviderKind::Anthropic)
        && snapshot.is_some_and(|(snapshot, age)| window_pressure_at(snapshot, age))
}

/// Pure core of [`oauth_window_pressure_for`].
fn window_pressure_at(snapshot: RateLimitSnapshot, age: Duration) -> bool {
    age <= QUOTA_SNAPSHOT_FRESH
        && binding_window(snapshot)
            .is_some_and(|(_, window)| window.utilization >= HEADROOM_UTIL_THRESHOLD)
}

/// The binding unified window for policy and display: the server-flagged
/// representative claim when present, else the hotter of the two windows.
/// Returns the window with its short display label (`"5h"` / `"7d"`).
#[must_use]
pub fn binding_window(snapshot: RateLimitSnapshot) -> Option<(&'static str, RateLimitWindow)> {
    match snapshot.representative {
        Some(RateLimitWindowKind::FiveHour) => snapshot.five_hour.map(|w| ("5h", w)),
        Some(RateLimitWindowKind::SevenDay) => snapshot.seven_day.map(|w| ("7d", w)),
        None => match (snapshot.five_hour, snapshot.seven_day) {
            (Some(five_hour), Some(seven_day)) => {
                Some(if five_hour.utilization >= seven_day.utilization {
                    ("5h", five_hour)
                } else {
                    ("7d", seven_day)
                })
            }
            (Some(five_hour), None) => Some(("5h", five_hour)),
            (None, Some(seven_day)) => Some(("7d", seven_day)),
            (None, None) => None,
        },
    }
}

/// Pure core of [`rate_limit_headroom_low`] — testable without touching the
/// process-global atomics (which other tests legitimately mark).
fn headroom_low_at(now_ms: u64, cooldown_until_ms: u64, last_rate_limit_at_ms: u64) -> bool {
    if cooldown_until_ms > now_ms {
        return true;
    }
    last_rate_limit_at_ms > 0
        && now_ms.saturating_sub(last_rate_limit_at_ms) < RATE_LIMIT_HEADROOM_WINDOW_MS
}

// ---------------------------------------------------------------------------
// Provider-agnostic quota views (P0-b)
// ---------------------------------------------------------------------------

/// One normalized quota row for the HUD/router: a single provider window
/// expressed as *remaining* headroom (never utilization). Anthropic rows are
/// measured (`estimated = false`) from the unified 5h/7d windows; every other
/// provider has no remaining-headroom header, so its row is *estimated* from
/// the 429 cool-down / recent-throttle state (`estimated = true`, which a HUD
/// renders as `(est)`).
///
/// Caveats baked into the builder: (a) utilization is inverted to remaining
/// (`remaining = 100 − used`); (b) a 429 `Retry-After` is a *cool-down* hint,
/// not a quota reset — only Anthropic's `resets_at_unix` is a true window reset;
/// (c) providers fold per [`provider_slot`], so a `DeepSeek` 429 shows up on the
/// shared OpenAI slot and a Grok 429 on the xAI slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderQuotaView {
    pub provider: ProviderKind,
    /// `"5h"` / `"7d"` for Anthropic's measured windows, `"429"` for an
    /// estimated (cool-down-derived) row.
    pub window_label: String,
    /// Remaining headroom percent (`0..=100`), NOT utilization. `None` when the
    /// figure is genuinely unknown (never invented).
    pub remaining_percent: Option<u8>,
    /// Absolute clock time the window resets, when known (Anthropic window
    /// reset, or the wall-clock end of an active 429 cool-down).
    pub resets_at_unix: Option<u64>,
    /// `true` when the row is inferred from 429 frequency rather than a measured
    /// remaining-headroom header — the HUD marks these `(est)`.
    pub estimated: bool,
    /// The model this row is scoped to, or `None` for the account-wide window
    /// (see [`QuotaWindow::model`]).
    ///
    /// **Policy readers must skip the scoped rows.** They are a display fact
    /// about ONE model, so folding them into a provider-wide judgement states
    /// something untrue: the router's headroom penalty would rank the whole
    /// provider by its most exhausted model, and the post-429 wait would hold
    /// the turn for a window the request is not billed against. Both callers
    /// filter on `model.is_none()`; [`crate::quota_probe`] is free to record
    /// every window the provider names because of that filter, not despite it.
    pub model: Option<String>,
}

/// Current cross-provider quota view: the measured Anthropic windows (when a
/// fresh snapshot exists) plus an estimated row per non-Anthropic provider that
/// is currently cooled-down or recently throttled. Providers with no signal are
/// omitted entirely — the caller sees "unknown", never a fabricated figure.
#[must_use]
pub fn provider_quota_views() -> Vec<ProviderQuotaView> {
    let now_mono = now_monotonic_millis();
    let now_unix = now_unix_secs();
    let mut views = Vec::new();
    for kind in [
        ProviderKind::Anthropic,
        ProviderKind::Xai,
        ProviderKind::OpenAi,
        ProviderKind::Google,
        ProviderKind::Ollama,
    ] {
        // Best signal first. A reading the provider gave us beats both the
        // response headers (which only exist after a request) and the 429
        // guess (which only exists after the damage), so it wins outright
        // rather than being merged with them into two rows saying different
        // things about one window.
        let measured = latest_measured_quota(kind)
            .map(|(quota, age)| measured_quota_views(kind, &quota, age, now_unix))
            .unwrap_or_default();
        if !measured.is_empty() {
            views.extend(measured);
            continue;
        }
        if kind == ProviderKind::Anthropic {
            // Anthropic alone carries remaining headroom on every response, so
            // it has a second real signal to fall back to before guessing.
            views.extend(
                latest_rate_limit_snapshot()
                    .map(|(snapshot, age)| anthropic_quota_views(snapshot, age, now_unix))
                    .unwrap_or_default(),
            );
            continue;
        }
        let slot = provider_slot(kind);
        if let Some(view) = estimated_quota_view(
            kind,
            now_mono,
            RATE_LIMIT_COOLDOWN_UNTIL_MS[slot].load(Ordering::Relaxed),
            RATE_LIMIT_COOLDOWN_UNTIL_UNIX_MS[slot].load(Ordering::Relaxed),
            LAST_RATE_LIMIT_AT_MS[slot].load(Ordering::Relaxed),
        ) {
            views.push(view);
        }
    }
    views
}

/// Pure core of the measured rows: drop a stale reading wholesale, and drop any
/// single window that has already rolled over — a reading still fresh by age
/// can carry a window whose reset passed seconds ago, and its old percentage
/// describes a window that no longer exists.
fn measured_quota_views(
    kind: ProviderKind,
    quota: &MeasuredQuota,
    age: Duration,
    now_unix: u64,
) -> Vec<ProviderQuotaView> {
    if age > MEASURED_QUOTA_FRESH {
        return Vec::new();
    }
    quota
        .windows
        .iter()
        .filter(|window| {
            window
                .resets_at_unix
                .is_none_or(|reset| reset > now_unix)
        })
        .map(|window| ProviderQuotaView {
            provider: kind,
            window_label: window.label.clone(),
            remaining_percent: Some(window.remaining_percent.min(100)),
            resets_at_unix: window.resets_at_unix,
            estimated: false,
            model: window.model.clone(),
        })
        .collect()
}

/// Pure core of the Anthropic measured rows: normalize each present unified
/// window to *remaining* headroom, dropping a stale snapshot wholesale and
/// discarding any single window whose reset time has already passed (a snapshot
/// still "fresh" by age can carry a window that reset seconds ago — its high
/// utilization is meaningless). `now_unix` is injected so this stays a pure
/// function of its inputs (no hidden clock read).
fn anthropic_quota_views(
    snapshot: RateLimitSnapshot,
    age: Duration,
    now_unix: u64,
) -> Vec<ProviderQuotaView> {
    if age > QUOTA_SNAPSHOT_FRESH {
        return Vec::new();
    }
    let mut views = Vec::new();
    for (label, window) in [("5h", snapshot.five_hour), ("7d", snapshot.seven_day)] {
        let Some(window) = window else {
            continue;
        };
        if window.resets_at_unix.is_some_and(|reset| reset <= now_unix) {
            continue;
        }
        views.push(ProviderQuotaView {
            provider: ProviderKind::Anthropic,
            window_label: label.to_string(),
            remaining_percent: Some(100u8.saturating_sub(window.used_percent())),
            resets_at_unix: window.resets_at_unix,
            estimated: false,
            // Response headers carry the account's two windows only; the
            // per-model allowances exist solely on the usage endpoint.
            model: None,
        });
    }
    views
}

/// Pure core of a non-Anthropic provider's estimated row from its 429 state: an
/// active cool-down reports `0%` remaining with the wall-clock reset instant; a
/// recent (but expired) 429 within [`RATE_LIMIT_HEADROOM_WINDOW_MS`] reports a
/// low fixed estimate with no reset; no signal yields no row (never invent an
/// unknown). `now_mono` and the atomics are injected so the mapping is testable
/// without touching the process globals.
fn estimated_quota_view(
    kind: ProviderKind,
    now_mono: u64,
    cooldown_until_mono: u64,
    cooldown_reset_unix_ms: u64,
    last_rate_limit_at_mono: u64,
) -> Option<ProviderQuotaView> {
    // `cooldown_until_mono` here is the ACCOUNT window's deadline only. A
    // provider overload parks its own separate deadline
    // ([`OVERLOAD_PAUSE_UNTIL_MS`]) precisely so it cannot reach this view:
    // reporting a 529 as "429 · 0 % remaining" would claim an exhausted plan on a
    // window measured at 2 %, and every consumer here (HUD gauge, router headroom
    // penalty, binding-window wait) would inherit the false figure.
    if cooldown_until_mono > now_mono {
        return Some(ProviderQuotaView {
            provider: kind,
            window_label: "429".to_string(),
            remaining_percent: Some(0),
            resets_at_unix: (cooldown_reset_unix_ms > 0).then_some(cooldown_reset_unix_ms / 1000),
            estimated: true,
            model: None,
        });
    }
    if last_rate_limit_at_mono > 0
        && now_mono.saturating_sub(last_rate_limit_at_mono) < RATE_LIMIT_HEADROOM_WINDOW_MS
    {
        return Some(ProviderQuotaView {
            provider: kind,
            window_label: "429".to_string(),
            remaining_percent: Some(RECENT_RATE_LIMIT_REMAINING_PERCENT),
            resets_at_unix: None,
            estimated: true,
            model: None,
        });
    }
    None
}

/// A window that reports this much (or less) remaining headroom is treated as
/// the one a hard 429 is binding on — headers round, so "exhausted" rarely
/// reads exactly `0`.
const RESET_WAIT_EXHAUSTED_PERCENT: u8 = 5;
/// Grace added on top of a computed reset wait so the retry lands after the
/// window actually rolls over, not a clock-skewed second before it.
const RESET_WAIT_GRACE: Duration = Duration::from_secs(10);

/// When the quota wall for `kind` is known to lift within `band`, the wait
/// (with a small grace) a caller should hold instead of falling back to
/// another provider — `None` otherwise.
///
/// Sources: the 429's explicit `retry_after` hint plus the freshest known
/// window resets for the provider ([`provider_quota_views`] — Anthropic's
/// measured windows, the 429 cool-down end elsewhere). Only windows that look
/// binding (estimated 429 rows, or measured rows at ≤5% remaining) count, and
/// when several bind the LATEST reset wins — requests keep failing until every
/// exhausted window clears. An unknown reset is never guessed: the caller
/// falls back exactly as before.
#[must_use]
pub fn reset_wait_within_band(
    kind: ProviderKind,
    retry_after: Option<Duration>,
    band: Duration,
) -> Option<Duration> {
    reset_wait_within_band_for(&provider_quota_views(), kind, retry_after, band, now_unix_secs())
}

/// Pure core of [`reset_wait_within_band`]: injected views and clock.
/// How long until the measured **account-wide** window of `kind` that is
/// exhausted (or estimated so) resets — the wait a refusal that names no reset
/// of its own inherits. `None` when no such window is known.
#[must_use]
pub fn measured_reset_wait(kind: ProviderKind) -> Option<Duration> {
    measured_reset_wait_for(&provider_quota_views(), kind, now_unix_secs())
}

fn measured_reset_wait_for(
    views: &[ProviderQuotaView],
    kind: ProviderKind,
    now_unix: u64,
) -> Option<Duration> {
    views
        .iter()
        .filter(|view| {
            // Account-wide rows only. A per-model window at 0% says one model is
            // spent, not that the request in hand is walled: holding the turn
            // until a sibling model's weekly window rolls over would park it for
            // days on a plan with headroom (see [`ProviderQuotaView::model`]).
            view.provider == kind
                && view.model.is_none()
                && (view.estimated
                    || view
                        .remaining_percent
                        .is_some_and(|remaining| remaining <= RESET_WAIT_EXHAUSTED_PERCENT))
        })
        .filter_map(|view| view.resets_at_unix)
        .map(|resets_at| Duration::from_secs(resets_at.saturating_sub(now_unix)))
        .max()
}

fn reset_wait_within_band_for(
    views: &[ProviderQuotaView],
    kind: ProviderKind,
    retry_after: Option<Duration>,
    band: Duration,
    now_unix: u64,
) -> Option<Duration> {
    if band.is_zero() {
        return None;
    }
    let view_wait = measured_reset_wait_for(views, kind, now_unix);
    let wait = match (retry_after.filter(|hint| !hint.is_zero()), view_wait) {
        (Some(hint), Some(view)) => hint.max(view),
        (Some(hint), None) => hint,
        (None, Some(view)) => view,
        (None, None) => return None,
    };
    (wait <= band).then(|| wait + RESET_WAIT_GRACE)
}

#[cfg(test)]
mod tests {
    use super::{
        anthropic_quota_views, binding_window, cooldown_wait_ms, estimated_quota_view,
        headroom_low_at, headroom_low_for_kind_at, mark_rate_limit_cooldown,
        mark_capacity_stall_from, measured_quota_views, provider_quota_views,
        rate_limit_backoff_ms, rate_limit_cooldown_remaining_ms, quota_fallback_permitted_at,
        OVERLOAD_COOLDOWN_MAX_MS, measured_reset_wait_for,
        reset_wait_within_band_for, window_pressure_at, MeasuredQuota, ProviderQuotaView,
        QuotaWindow, MEASURED_QUOTA_FRESH, QUOTA_SNAPSHOT_FRESH, RATE_LIMIT_HEADROOM_WINDOW_MS,
        RESET_WAIT_GRACE,
    };
    use crate::{CapacityScope, ProviderKind};
    use core_types::{RateLimitSnapshot, RateLimitWindow, RateLimitWindowKind};
    use std::time::Duration;

    /// 빈 스냅샷(API-key 응답)은 기록을 덮지 않고, 데이터 있는 스냅샷은
    /// 최신값으로 교체된다.
    #[test]
    fn empty_snapshots_never_mask_a_subscription_reading() {
        let five_hour = |utilization: f64| RateLimitSnapshot {
            five_hour: Some(RateLimitWindow {
                utilization,
                resets_at_unix: None,
            }),
            seven_day: None,
            representative: None,
        };
        super::record_rate_limit_snapshot(five_hour(0.4));
        super::record_rate_limit_snapshot(RateLimitSnapshot::default());
        let (snapshot, age) = super::latest_rate_limit_snapshot().expect("recorded");
        assert!(
            (snapshot.five_hour.expect("window").utilization - 0.4).abs() < f64::EPSILON,
            "blank API-key snapshot must not mask the subscription reading"
        );
        assert!(age.as_secs() < 60);
        super::record_rate_limit_snapshot(five_hour(0.9));
        let (snapshot, _) = super::latest_rate_limit_snapshot().expect("recorded");
        assert!((snapshot.five_hour.expect("window").utilization - 0.9).abs() < f64::EPSILON);
    }

    /// 공식 Anthropic SDK 패리티: 양수 Retry-After 힌트는 그대로 신뢰(캡 한정),
    /// 지수 사다리는 힌트가 없을 때만. 종전 max(힌트, 사다리) 플로어는 작은
    /// 힌트를 최대 8배 과대기시켜 starvation을 연장했다.
    #[test]
    fn cooldown_trusts_positive_server_hint_verbatim() {
        // 힌트 5s, 사다리 120s(스텝 3) → 힌트 그대로.
        assert_eq!(cooldown_wait_ms(Some(Duration::from_secs(5)), 3), 5_000);
        // 힌트 없음 → 지수 사다리.
        assert_eq!(cooldown_wait_ms(None, 3), 120_000);
        assert_eq!(cooldown_wait_ms(None, 0), 15_000);
        // 적대적/과대 힌트는 캡.
        assert_eq!(cooldown_wait_ms(Some(Duration::from_secs(300)), 0), 120_000);
        // 0 힌트는 무의미 → 사다리 폴백.
        assert_eq!(cooldown_wait_ms(Some(Duration::ZERO), 1), 30_000);
        // backoff 사다리 자체.
        assert_eq!(rate_limit_backoff_ms(0), 15_000);
    }

    /// W9-4 선행 신호의 순수 코어: 신선한 스냅샷에서 binding 윈도우(대표 클레임
    /// 우선, 없으면 더 뜨거운 쪽)가 임계 이상일 때만 압력으로 판정. stale·빈
    /// 스냅샷·임계 미만은 통과.
    #[test]
    fn oauth_window_pressure_uses_binding_window_and_freshness() {
        let window = |utilization: f64| {
            Some(RateLimitWindow {
                utilization,
                resets_at_unix: None,
            })
        };
        let fresh = Duration::from_secs(30);
        // 대표 클레임이 5h를 지목 → 5h 95%로 압력.
        let snapshot = RateLimitSnapshot {
            five_hour: window(0.95),
            seven_day: window(0.10),
            representative: Some(RateLimitWindowKind::FiveHour),
        };
        assert!(window_pressure_at(snapshot, fresh));
        // 같은 수치라도 대표가 한가한 7d를 지목하면 통과.
        let snapshot = RateLimitSnapshot {
            representative: Some(RateLimitWindowKind::SevenDay),
            ..snapshot
        };
        assert!(!window_pressure_at(snapshot, fresh));
        // 대표 부재 → 더 뜨거운 윈도우(max) 기준.
        let snapshot = RateLimitSnapshot {
            representative: None,
            ..snapshot
        };
        assert!(window_pressure_at(snapshot, fresh));
        // 임계(95%) 미만·빈 스냅샷·stale 스냅샷은 전부 통과.
        let cool = RateLimitSnapshot {
            five_hour: window(0.94),
            seven_day: None,
            representative: None,
        };
        assert!(!window_pressure_at(cool, fresh));
        assert!(!window_pressure_at(RateLimitSnapshot::default(), fresh));
        let hot = RateLimitSnapshot {
            five_hour: window(0.99),
            seven_day: None,
            representative: None,
        };
        assert!(!window_pressure_at(
            hot,
            QUOTA_SNAPSHOT_FRESH + Duration::from_secs(1)
        ));
        // binding_window 라벨/선택 계약.
        assert_eq!(binding_window(cool).expect("binding").0, "5h");
    }

    #[test]
    fn quota_fallback_gate_uses_fresh_anthropic_utilization_or_unknown_state() {
        let snapshot = |utilization: f64| RateLimitSnapshot {
            five_hour: Some(RateLimitWindow {
                utilization,
                resets_at_unix: None,
            }),
            seven_day: None,
            representative: Some(RateLimitWindowKind::FiveHour),
        };
        let fresh = Duration::from_secs(30);

        assert!(!quota_fallback_permitted_at(
            ProviderKind::Anthropic,
            Some((snapshot(0.949), fresh)),
            None,
        ));
        assert!(quota_fallback_permitted_at(
            ProviderKind::Anthropic,
            Some((snapshot(0.95), fresh)),
            None,
        ));
        assert!(quota_fallback_permitted_at(
            ProviderKind::Anthropic,
            Some((
                snapshot(0.99),
                QUOTA_SNAPSHOT_FRESH + Duration::from_secs(1),
            )),
            None,
        ));
        assert!(quota_fallback_permitted_at(
            ProviderKind::Anthropic,
            None,
            None,
        ));
        assert!(quota_fallback_permitted_at(
            ProviderKind::Anthropic,
            None,
            Some((999, QUOTA_SNAPSHOT_FRESH + Duration::from_secs(1))),
        ));

        assert!(quota_fallback_permitted_at(
            ProviderKind::Anthropic,
            Some((snapshot(0.20), fresh)),
            Some((950, fresh)),
        ));
        assert!(!quota_fallback_permitted_at(
            ProviderKind::Anthropic,
            None,
            Some((949, fresh)),
        ));
        assert!(quota_fallback_permitted_at(
            ProviderKind::OpenAi,
            Some((snapshot(0.20), fresh)),
            Some((100, fresh)),
        ));
    }

    /// 헤드룸 게이트의 순수 코어: 쿨다운 활성·최근 429만 low로 판정하고,
    /// 무이력(0)·윈도우 밖 이력은 통과시킨다.
    #[test]
    fn headroom_gate_tracks_cooldown_and_recent_throttle() {
        // 이력 전무 → 헤드룸 정상.
        assert!(!headroom_low_at(1_000_000, 0, 0));
        // 쿨다운이 아직 살아 있으면 low.
        assert!(headroom_low_at(1_000_000, 1_000_001, 0));
        // 쿨다운은 끝났지만 429가 윈도우 안 → low.
        let recent = 1_000_000 - (RATE_LIMIT_HEADROOM_WINDOW_MS - 1);
        assert!(headroom_low_at(1_000_000, 999_999, recent));
        // 429가 윈도우 밖으로 밀려나면 회복.
        let stale = 1_000_000 - RATE_LIMIT_HEADROOM_WINDOW_MS;
        assert!(!headroom_low_at(1_000_000, 999_999, stale));
    }

    #[test]
    fn anthropic_quota_pressure_does_not_throttle_other_providers() {
        let hot_anthropic_snapshot = || {
            Some((
                RateLimitSnapshot {
                    five_hour: Some(RateLimitWindow {
                        utilization: 0.95,
                        resets_at_unix: None,
                    }),
                    seven_day: None,
                    representative: Some(RateLimitWindowKind::FiveHour),
                },
                Duration::from_secs(30),
            ))
        };
        let now = 1_000_000;
        assert!(
            headroom_low_for_kind_at(ProviderKind::Anthropic, now, 0, 0, hot_anthropic_snapshot()),
            "Anthropic should honor its own hot unified quota snapshot"
        );
        assert!(
            !headroom_low_for_kind_at(ProviderKind::OpenAi, now, 0, 0, hot_anthropic_snapshot()),
            "Anthropic unified quota pressure must not throttle OpenAI-compatible headroom"
        );
    }

    /// P0-b 순수 코어: Anthropic 측정 윈도우는 사용률→잔량으로 반전, 이미 리셋
    /// 지난 창은 폐기, stale 스냅샷은 전부 폐기.
    #[test]
    fn anthropic_views_invert_utilization_and_drop_reset_windows() {
        let fresh = Duration::from_secs(30);
        let now_unix = 10_000;
        let snapshot = RateLimitSnapshot {
            five_hour: Some(RateLimitWindow {
                utilization: 0.30,
                resets_at_unix: Some(now_unix + 600),
            }),
            seven_day: Some(RateLimitWindow {
                utilization: 0.90,
                resets_at_unix: Some(now_unix + 6_000),
            }),
            representative: Some(RateLimitWindowKind::FiveHour),
        };
        let views = anthropic_quota_views(snapshot, fresh, now_unix);
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].window_label, "5h");
        assert_eq!(views[0].remaining_percent, Some(70), "100 − 30% used");
        assert!(!views[0].estimated);
        assert_eq!(views[0].resets_at_unix, Some(now_unix + 600));
        assert_eq!(views[1].window_label, "7d");
        assert_eq!(views[1].remaining_percent, Some(10), "100 − 90% used");

        // A window whose reset already passed is discarded (audit trap #4).
        let already_reset = RateLimitSnapshot {
            five_hour: Some(RateLimitWindow {
                utilization: 0.99,
                resets_at_unix: Some(now_unix - 1),
            }),
            seven_day: None,
            representative: None,
        };
        assert!(anthropic_quota_views(already_reset, fresh, now_unix).is_empty());

        // A stale snapshot is dropped wholesale.
        assert!(anthropic_quota_views(
            snapshot,
            QUOTA_SNAPSHOT_FRESH + Duration::from_secs(1),
            now_unix
        )
        .is_empty());
    }

    /// A measured reading is the provider's own answer, so it must survive as
    /// `estimated: false` — the sidebar keys its `est` tag off exactly that
    /// flag, and a real figure wearing a guess's label teaches the wrong thing.
    #[test]
    fn measured_views_carry_the_provider_figure_unestimated() {
        let now_unix = 1_000_000;
        let quota = MeasuredQuota {
            windows: vec![
                QuotaWindow {
                    label: "5h".to_string(),
                    remaining_percent: 58,
                    resets_at_unix: Some(now_unix + 3_600),
                    model: None,
                },
                QuotaWindow {
                    label: "7d".to_string(),
                    remaining_percent: 91,
                    resets_at_unix: None,
                    model: None,
                },
                QuotaWindow {
                    label: "7d".to_string(),
                    remaining_percent: 0,
                    resets_at_unix: Some(now_unix + 7_200),
                    model: Some("GPT-5.3-Codex-Spark".to_string()),
                },
            ],
            account: Some("someone@example.com".to_string()),
        };

        let views = measured_quota_views(
            ProviderKind::OpenAi,
            &quota,
            Duration::from_secs(30),
            now_unix,
        );

        assert_eq!(views.len(), 3);
        assert_eq!(views[0].window_label, "5h");
        assert_eq!(views[0].remaining_percent, Some(58));
        assert_eq!(views[0].resets_at_unix, Some(now_unix + 3_600));
        assert!(views.iter().all(|view| !view.estimated));
        assert!(views
            .iter()
            .all(|view| view.provider == ProviderKind::OpenAi));
        // The per-model allowance travels as its own row rather than being
        // folded into the account window it sits beside.
        assert_eq!(views[0].model, None);
        assert_eq!(views[2].model.as_deref(), Some("GPT-5.3-Codex-Spark"));
    }

    /// Two ways a reading stops describing reality: the poller died and left it
    /// behind, or one window rolled over while the rest stayed valid. Reporting
    /// either invites spending against headroom that is already gone.
    #[test]
    fn measured_views_drop_stale_readings_and_rolled_over_windows() {
        let now_unix = 1_000_000;
        let quota = MeasuredQuota {
            windows: vec![
                QuotaWindow {
                    label: "5h".to_string(),
                    remaining_percent: 12,
                    resets_at_unix: Some(now_unix - 1),
                    model: None,
                },
                QuotaWindow {
                    label: "7d".to_string(),
                    remaining_percent: 40,
                    resets_at_unix: Some(now_unix + 60),
                    model: None,
                },
            ],
            account: None,
        };

        // The expired window goes; its sibling stays.
        let fresh = measured_quota_views(
            ProviderKind::Google,
            &quota,
            Duration::from_secs(1),
            now_unix,
        );
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].window_label, "7d");

        // A reading nobody has refreshed is dropped whole.
        assert!(measured_quota_views(
            ProviderKind::Google,
            &quota,
            MEASURED_QUOTA_FRESH + Duration::from_secs(1),
            now_unix,
        )
        .is_empty());
    }

    /// P0-b 순수 코어: 비-Anthropic 추정 행 — 활성 쿨다운=0%+리셋, 최근 429=낮은
    /// 상수+리셋 없음, 무신호=행 없음.
    #[test]
    fn estimated_view_reflects_cooldown_then_recent_then_silence() {
        let now = 1_000_000;
        // Active cool-down: 0% remaining with a wall-clock reset.
        let active = estimated_quota_view(ProviderKind::OpenAi, now, now + 5_000, 42_000, now)
            .expect("active cool-down row");
        assert_eq!(active.remaining_percent, Some(0));
        assert_eq!(active.window_label, "429");
        assert!(active.estimated);
        assert_eq!(active.resets_at_unix, Some(42), "42_000ms → 42s");
        // Recent (expired) 429 within the window: low fixed estimate, no reset.
        let recent = estimated_quota_view(ProviderKind::Google, now, now, 0, now - 1_000)
            .expect("recent-throttle row");
        assert_eq!(recent.remaining_percent, Some(10));
        assert_eq!(recent.resets_at_unix, None);
        assert!(recent.estimated);
        // No signal at all → no row.
        assert!(estimated_quota_view(ProviderKind::Xai, now, 0, 0, 0).is_none());
        // A 429 pushed outside the headroom window → no row.
        assert!(estimated_quota_view(
            ProviderKind::Xai,
            now,
            0,
            0,
            now - RATE_LIMIT_HEADROOM_WINDOW_MS
        )
        .is_none());
    }

    /// 이전된 마킹/쿨다운 상태의 통합 검증(위임 표면 포함): Ollama 슬롯에 쿨다운을
    /// 걸면 `provider_quota_views`에 0% 추정 행이 실측 unix 리셋과 함께 나타난다.
    /// Ollama 슬롯은 다른 api 테스트가 건드리지 않아 프로세스 전역 오염이 없다.
    #[test]
    fn marking_a_cooldown_surfaces_an_estimated_view_end_to_end() {
        mark_rate_limit_cooldown(ProviderKind::Ollama, 60_000);
        assert!(
            rate_limit_cooldown_remaining_ms(ProviderKind::Ollama) > 1_000,
            "the marked provider shows a live cool-down"
        );
        let views = provider_quota_views();
        let ollama: Vec<&ProviderQuotaView> = views
            .iter()
            .filter(|view| view.provider == ProviderKind::Ollama)
            .collect();
        assert_eq!(ollama.len(), 1, "exactly one estimated Ollama row");
        assert_eq!(ollama[0].remaining_percent, Some(0));
        assert!(ollama[0].estimated);
        assert!(
            ollama[0].resets_at_unix.is_some_and(|reset| reset > 0),
            "an active cool-down carries a wall-clock reset from the recorded unix deadline"
        );
    }

    /// 마킹 시맨틱 보존: Retry-After 힌트가 지수 사다리를 이긴다(작은 힌트도
    /// 그대로). 마킹이 패닉 없이 동작함을 함께 확인. Xai 슬롯 전용(격리).
    #[test]
    fn cooldown_from_prefers_server_hint_and_marks_without_panic() {
        assert_eq!(rate_limit_backoff_ms(0), 15_000);
        mark_capacity_stall_from(
            ProviderKind::Xai,
            CapacityScope::Account,
            Some(Duration::from_secs(60)),
            0,
        );
        assert!(rate_limit_cooldown_remaining_ms(ProviderKind::Xai) > 1_000);
    }

    /// A provider overload pauses briefly and leaves the ACCOUNT's window alone.
    ///
    /// The account ladder would have escalated this same attempt index to the
    /// 120 s ceiling and stamped "recently throttled" — which is what parked a
    /// 2 %-utilized account for two minutes and depressed sub-agent admission for
    /// five. Google slot only (isolated from the other cases here).
    #[test]
    fn overload_cooldown_is_short_and_never_charged_to_the_account() {
        // Deep into the ladder, where an account 429 would sit at the 120 s cap.
        let deep_attempt = 6;
        assert_eq!(rate_limit_backoff_ms(deep_attempt), 120_000);

        mark_capacity_stall_from(ProviderKind::Google, CapacityScope::Provider, None, deep_attempt);

        let remaining = rate_limit_cooldown_remaining_ms(ProviderKind::Google);
        assert!(remaining > 0, "a shedding provider still gets a brief pause");
        assert!(
            remaining <= OVERLOAD_COOLDOWN_MAX_MS,
            "overload cool-down must not climb the account ladder: {remaining}ms"
        );
        // The estimated view answers "how much of the ACCOUNT's quota is left,
        // inferred from 429s". A provider overload contributes nothing to that,
        // so it must not synthesize a `429 · 0 % remaining` row — the figure the
        // HUD gauge and the router headroom penalty would then believe.
        assert!(
            !provider_quota_views()
                .iter()
                .any(|view| view.provider == ProviderKind::Google),
            "a provider overload must not claim the account is out of quota"
        );
    }

    fn view(
        kind: ProviderKind,
        label: &str,
        remaining: Option<u8>,
        resets_at: Option<u64>,
        estimated: bool,
    ) -> ProviderQuotaView {
        ProviderQuotaView {
            provider: kind,
            window_label: label.to_string(),
            remaining_percent: remaining,
            resets_at_unix: resets_at,
            estimated,
            model: None,
        }
    }

    fn model_view(
        kind: ProviderKind,
        model: &str,
        label: &str,
        remaining: Option<u8>,
        resets_at: Option<u64>,
    ) -> ProviderQuotaView {
        ProviderQuotaView {
            model: Some(model.to_string()),
            ..view(kind, label, remaining, resets_at, false)
        }
    }

    /// 대기 밴드 선택: 소진 창(≤5% 잔량)의 리셋이 밴드 내면 대기시간(+grace),
    /// 밴드를 넘으면 None(폴백 유지). 소진 아닌 창의 이른 리셋은 무시된다 —
    /// 그걸 기다려도 벽은 안 걷힌다.
    #[test]
    fn reset_wait_holds_only_when_a_binding_window_clears_inside_the_band() {
        let now = 1_000_000u64;
        let band = Duration::from_secs(15 * 60);
        // 5h 창 소진(0%), 8분 뒤 리셋 → 밴드 내 대기.
        let views = [view(ProviderKind::Anthropic, "5h", Some(0), Some(now + 480), false)];
        assert_eq!(
            reset_wait_within_band_for(&views, ProviderKind::Anthropic, None, band, now),
            Some(Duration::from_secs(480) + RESET_WAIT_GRACE)
        );
        // 7d 창 소진, 580분 뒤 리셋 → 밴드 밖, 폴백.
        let views = [view(ProviderKind::Anthropic, "7d", Some(0), Some(now + 580 * 60), false)];
        assert_eq!(
            reset_wait_within_band_for(&views, ProviderKind::Anthropic, None, band, now),
            None
        );
        // 5h는 멀쩡(40% 잔량)하고 리셋만 이른 경우 → 바인딩 아님, 대기 없음.
        let views = [
            view(ProviderKind::Anthropic, "5h", Some(40), Some(now + 60), false),
            view(ProviderKind::Anthropic, "7d", Some(0), Some(now + 580 * 60), false),
        ];
        assert_eq!(
            reset_wait_within_band_for(&views, ProviderKind::Anthropic, None, band, now),
            None
        );
        // 두 창 모두 소진이면 늦은 리셋이 이긴다(둘 다 걷혀야 통과).
        let views = [
            view(ProviderKind::Anthropic, "5h", Some(0), Some(now + 120), false),
            view(ProviderKind::Anthropic, "7d", Some(2), Some(now + 600), false),
        ];
        assert_eq!(
            reset_wait_within_band_for(&views, ProviderKind::Anthropic, None, band, now),
            Some(Duration::from_secs(600) + RESET_WAIT_GRACE)
        );
    }

    /// `retry_after` 힌트·타 프로바이더 행·estimated 행·밴드 0의 경계 계약.
    #[test]
    fn reset_wait_honors_hints_estimated_rows_and_the_disable_band() {
        let now = 2_000_000u64;
        let band = Duration::from_secs(15 * 60);
        // 뷰가 없어도 서버 retry_after 힌트가 밴드 내면 대기.
        assert_eq!(
            reset_wait_within_band_for(&[], ProviderKind::OpenAi, Some(Duration::from_secs(90)), band, now),
            Some(Duration::from_secs(90) + RESET_WAIT_GRACE)
        );
        // 힌트와 뷰가 다르면 보수적으로 늦은 쪽을 기다린다.
        let views = [view(ProviderKind::OpenAi, "429", None, Some(now + 300), true)];
        assert_eq!(
            reset_wait_within_band_for(&views, ProviderKind::OpenAi, Some(Duration::from_secs(60)), band, now),
            Some(Duration::from_secs(300) + RESET_WAIT_GRACE)
        );
        // 타 프로바이더의 소진 행은 무관.
        let views = [view(ProviderKind::Google, "429", Some(0), Some(now + 60), true)];
        assert_eq!(
            reset_wait_within_band_for(&views, ProviderKind::OpenAi, None, band, now),
            None
        );
        // 리셋 정보가 전무하면 절대 발명하지 않는다.
        assert_eq!(
            reset_wait_within_band_for(&[], ProviderKind::Anthropic, None, band, now),
            None
        );
        // 밴드 0 = 기능 비활성.
        assert_eq!(
            reset_wait_within_band_for(&[], ProviderKind::OpenAi, Some(Duration::from_secs(5)), Duration::ZERO, now),
            None
        );
    }

    /// 모델별 창은 표시용이다 — 그 창이 0% 라도 이번 요청이 그것에 청구되는 것은
    /// 아니므로 대기 판단에 끼면 안 된다. 실측(2026-08-27 `wham/usage`)에서
    /// codex 계정 주간 창은 5% 남아 8분 뒤 리셋인데 모델별 주간 창은 다음 주에
    /// 리셋된다: 섞으면 밴드를 넘겨 정당한 짧은 대기가 통째로 사라진다.
    #[test]
    fn a_per_model_window_never_decides_the_post_429_wait() {
        let now = 3_000_000u64;
        let band = Duration::from_secs(15 * 60);
        let account = view(ProviderKind::OpenAi, "7d", Some(0), Some(now + 480), false);
        let scoped = model_view(
            ProviderKind::OpenAi,
            "GPT-5.3-Codex-Spark",
            "7d",
            Some(0),
            Some(now + 7 * 24 * 3_600),
        );

        assert_eq!(
            reset_wait_within_band_for(
                &[account, scoped.clone()],
                ProviderKind::OpenAi,
                None,
                band,
                now
            ),
            Some(Duration::from_secs(480) + RESET_WAIT_GRACE),
            "the account window still decides when a scoped row sits beside it"
        );
        // 모델별 행만 있으면 기다릴 근거가 없다.
        assert_eq!(
            reset_wait_within_band_for(&[scoped], ProviderKind::OpenAi, None, band, now),
            None
        );
    }

    /// The measured reset a silent refusal inherits: the exhausted (or
    /// estimated) account-wide window of that provider, never a scoped row,
    /// never another provider, and the latest of several.
    #[test]
    fn a_measured_reset_wait_is_the_exhausted_account_window_of_that_provider() {
        let now = 3_000_000u64;
        let exhausted = view(ProviderKind::OpenAi, "5h", Some(3), Some(now + 8_220), false);
        let weekly = view(ProviderKind::OpenAi, "7d", Some(2), Some(now + 90_000), false);
        let roomy = view(ProviderKind::OpenAi, "5h", Some(60), Some(now + 100), false);
        let other = view(ProviderKind::Anthropic, "5h", Some(0), Some(now + 5), false);
        let scoped = model_view(ProviderKind::OpenAi, "spark", "7d", Some(0), Some(now + 9));

        assert_eq!(
            measured_reset_wait_for(&[exhausted.clone(), other, scoped], ProviderKind::OpenAi, now),
            Some(Duration::from_secs(8_220))
        );
        assert_eq!(
            measured_reset_wait_for(&[exhausted, weekly], ProviderKind::OpenAi, now),
            Some(Duration::from_secs(90_000)),
            "the latest exhausted window is the wait"
        );
        assert_eq!(measured_reset_wait_for(&[roomy], ProviderKind::OpenAi, now), None);
        assert_eq!(
            measured_reset_wait_for(
                &[view(ProviderKind::OpenAi, "5h", None, Some(now + 30), true)],
                ProviderKind::OpenAi,
                now
            ),
            Some(Duration::from_secs(30)),
            "an estimated row counts as exhausted"
        );
    }
}
