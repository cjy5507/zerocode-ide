//! Cross-process rate-limit coordination for account-shared providers.
//!
//! zo reuses Claude Code's OAuth credentials (`Using Claude Code session
//! credentials`), so every zo process on the machine draws on ONE Anthropic
//! account rate limit. The per-process AIMD governor
//! (`tools::agent_tools::rate_limit`) throttles *within* a session, but several
//! concurrent sessions each believe they are under the limit and collectively
//! blow it — the account-tier 429 storm a lone Claude Code never provokes.
//!
//! This module shares the cool-down deadline and the account's reported
//! utilization through one small file per provider under the config home, so:
//!   - one process's 429 parks EVERY process until the shared window lifts
//!     (reactive), and
//!   - as the account's utilization climbs from all processes' combined load,
//!     every governor's headroom gate sees it and collapses admission to serial
//!     before the wall (proactive).
//!
//! The monotonic cool-down clock stays authoritative in-process (immune to NTP
//! steps); only the wall-clock (unix-ms) deadline is portable across processes,
//! so that is what is shared. Writes are atomic (temp + rename) and the
//! cool-down field only ratchets forward; the read-modify-write across processes
//! is lock-free and so mildly racy, but every value is a bounded, self-healing
//! deadline (≤ [`SHARED_COOLDOWN_MAX_MS`]) — a lost update costs at most one
//! window and the next 429 re-marks it. All I/O failures degrade to a no-op /
//! empty read so coordination can never wedge a turn.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::sync_bridge::lock_recovered;
use crate::ProviderKind;

/// Off-switch. Default on; set to `0`/`false`/`off`/`no` to disable all
/// cross-process sharing (each process falls back to its own governor).
const ENABLE_ENV: &str = "ZO_SHARED_RATE_COORD";

/// Utilization (×1000) at/above which the shared account window counts as
/// headroom-low, mirroring [`crate::quota::HEADROOM_UTIL_THRESHOLD`] (0.95).
pub(crate) const SHARED_UTIL_THRESHOLD_X1000: u64 = 950;

/// Max age of a shared utilization reading still trusted as a leading signal,
/// mirroring `crate::quota::QUOTA_SNAPSHOT_FRESH` (10 minutes). A stale hot
/// reading may already have reset, so it is ignored rather than throttling
/// forever.
const SHARED_UTIL_FRESH_MS: u64 = 600_000;

/// Upper bound on any shared cool-down window, matching the in-process
/// `RATE_LIMIT_COOLDOWN_MAX_MS`. Bounds the blast radius of a racy write.
const SHARED_COOLDOWN_MAX_MS: u64 = 120_000;

/// How long after the last shared 429 the account still counts as "recently
/// throttled" for the headroom gate, matching the in-process
/// `RATE_LIMIT_HEADROOM_WINDOW_MS` (5 minutes).
const SHARED_RECENT_429_WINDOW_MS: u64 = 300_000;

/// Minimum spacing between this process's utilization writes, so a busy stream
/// of responses cannot turn the shared file into a write hot spot. Cool-down
/// writes ignore this (429s are rare and must propagate immediately).
const UTIL_WRITE_MIN_INTERVAL_MS: u64 = 2_000;

/// Read cache TTL: a query re-reads the file at most this often per provider, so
/// admission/retry hot paths never stat+parse on every call.
const READ_CACHE_TTL_MS: u64 = 1_000;

/// Providers whose credentials are account-shared across zo processes.
///
/// The question is "do this machine's zo processes draw down one upstream
/// window?", not "is the credential globally shared". Anthropic reuses the
/// Claude Code OAuth and OpenAI reuses one ChatGPT/API credential, so in both
/// cases a 429 observed by one process is a fact for all of them.
///
/// OpenAI matters specifically because it serves the cross-model VERIFY leg.
/// Without coordination its exhaustion is only ever remembered in the process
/// that hit it, so every new session starts blind and re-earns the same 429 —
/// the "still rate limited but keeps trying" loop. The remaining providers are
/// keyed per deployment (self-hosted or per-project credentials), where one
/// process's window says nothing about another's.
const fn coordinated(kind: ProviderKind) -> bool {
    matches!(kind, ProviderKind::Anthropic | ProviderKind::OpenAi)
}

/// Process-level off switch set by
/// [`crate::quota::isolate_rate_limit_state_for_tests`], checked ahead of the
/// env var. The `cfg!(test)` default below cannot see a *downstream* crate's
/// test harness: when `api` is built as a dependency of another crate's test
/// binary it compiles without `--test`, so that arm reads `false` and the
/// shared file under the real config home goes live inside unit tests.
static FORCE_DISABLED: AtomicBool = AtomicBool::new(false);

/// Disable cross-process coordination for the rest of this process. One-way on
/// purpose: a test that isolated itself must not be re-enabled by a sibling
/// test running concurrently in the same binary.
pub(crate) fn force_disable_for_process() {
    FORCE_DISABLED.store(true, Ordering::Release);
}

/// Whether this process is a Cargo test binary.
///
/// `cfg!(test)` answers that only for THIS crate's own harness; when `api` is
/// linked into another crate's test binary it compiles without `--test`, so the
/// default arm read "production" and a unit test's cool-down mark landed in the
/// account-global file every running zo reads. Measured: a suite run parked the
/// developer's real Anthropic account for 60 s and stamped the 5-minute
/// "recently throttled" penalty on their live sessions.
///
/// Cargo builds every test, bench, and example target into `…/target/<profile>/
/// deps/`, while a shipped binary is `…/target/<profile>/zo` or an installed
/// copy — so the parent directory name is the discriminator, and it needs no
/// cooperation from the test author. Per-call-site opt-in cannot cover tests
/// that do not know they touch this state (the leak was found in three of them,
/// and a fourth arrived in a sibling branch the same day); this closes the class.
///
/// Failing open is the safe direction: a false positive only degrades a *test*
/// to per-process throttling, never production, because a shipped binary is not
/// in `deps/`. An explicit [`ENABLE_ENV`] value still overrides in both directions.
fn running_under_test_harness() -> bool {
    static UNDER_TEST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *UNDER_TEST.get_or_init(|| {
        std::env::current_exe().is_ok_and(|exe| {
            exe.parent()
                .and_then(std::path::Path::file_name)
                .is_some_and(|dir| dir == "deps")
        })
    })
}

fn enabled() -> bool {
    if FORCE_DISABLED.load(Ordering::Acquire) {
        return false;
    }
    match std::env::var(ENABLE_ENV) {
        // An explicit value decides: any falsey token disables, anything else on.
        Ok(raw) => !matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no" | ""
        ),
        // Unset: on in production, off under any test harness — this crate's own
        // (`cfg!(test)`) or a downstream crate's (see
        // [`running_under_test_harness`]). The shared file lives under the config
        // home, so leaving it on by default let any unrelated test that records a
        // snapshot/429 write to the real `~/.zo` — the well-known test-pollution
        // landmine, here with teeth: the record throttles live sessions.
        // `quota_shared`'s own tests opt in explicitly under an isolated
        // `ZO_CONFIG_HOME`.
        Err(_) => !cfg!(test) && !running_under_test_harness(),
    }
}

fn active(kind: ProviderKind) -> bool {
    coordinated(kind) && enabled()
}

/// Wall-clock unix milliseconds. A pre-epoch clock degrades to `0` (the file is
/// advisory; a bogus time can only mis-time coordination, never panic).
fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The shared record for one provider. Absent fields are `0` ("none").
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct SharedQuota {
    /// Wall-clock unix ms before which no request to this account should go out.
    cooldown_until_unix_ms: u64,
    /// Wall-clock unix ms of the most recent shared 429 (`0` = never).
    last_rate_limit_unix_ms: u64,
    /// Representative-window utilization ×1000 (`0` = unknown).
    utilization_x1000: u64,
    /// Wall-clock unix ms the utilization was last written.
    util_updated_unix_ms: u64,
}

impl SharedQuota {
    fn serialize(self) -> String {
        format!(
            "v1 {} {} {} {}\n",
            self.cooldown_until_unix_ms,
            self.last_rate_limit_unix_ms,
            self.utilization_x1000,
            self.util_updated_unix_ms,
        )
    }

    /// Parse the single-line `v1 <a> <b> <c> <d>` record. Any shape mismatch
    /// (older/newer version, truncation, garbage) yields `None` so the caller
    /// treats it as "no shared state" rather than trusting half a record.
    fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.split_whitespace();
        if parts.next()? != "v1" {
            return None;
        }
        let mut next = || parts.next().and_then(|token| token.parse::<u64>().ok());
        let quota = Self {
            cooldown_until_unix_ms: next()?,
            last_rate_limit_unix_ms: next()?,
            utilization_x1000: next()?,
            util_updated_unix_ms: next()?,
        };
        parts.next().is_none().then_some(quota)
    }
}

fn shared_path(kind: ProviderKind) -> PathBuf {
    let name = match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Xai => "xai",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Google => "google",
        ProviderKind::Ollama => "ollama",
    };
    core_types::paths::default_config_home()
        .join("rate")
        .join(format!("{name}.v1"))
}

/// Atomic write: a per-process temp file renamed over the target, so a reader
/// never observes a partial line. Best-effort — any error is swallowed.
fn atomic_write(path: &Path, contents: &str) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("rate"),
        std::process::id()
    ));
    if std::fs::write(&tmp, contents).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Fresh read straight from disk (bypasses the query cache), for the
/// read-modify-write on the write paths so a util write preserves the freshest
/// cool-down another process may have just marked.
fn read_from_disk(kind: ProviderKind) -> SharedQuota {
    let primary = shared_path(kind);
    let Some(file_name) = primary.file_name() else {
        return SharedQuota::default();
    };
    // Canonical roots can change as environment overrides are added or removed.
    // Reduce every valid copy instead of letting a valid but older primary hide
    // a lower-root 429. Cool-down and 429 timestamps are independent ratchets;
    // utilization is one timestamped snapshot, so keep its value paired with
    // the newest update timestamp.
    let roots = core_types::paths::zo_global_config_roots();
    let roots = if roots.is_empty() {
        vec![core_types::paths::default_config_home()]
    } else {
        roots
    };
    roots
        .into_iter()
        .filter_map(|root| {
            std::fs::read_to_string(root.join("rate").join(file_name))
                .ok()
                .and_then(|raw| SharedQuota::parse(&raw))
        })
        .fold(SharedQuota::default(), reduce_quota)
}

fn reduce_quota(mut reduced: SharedQuota, candidate: SharedQuota) -> SharedQuota {
    reduced.cooldown_until_unix_ms = reduced
        .cooldown_until_unix_ms
        .max(candidate.cooldown_until_unix_ms);
    reduced.last_rate_limit_unix_ms = reduced
        .last_rate_limit_unix_ms
        .max(candidate.last_rate_limit_unix_ms);
    if candidate.util_updated_unix_ms > reduced.util_updated_unix_ms {
        reduced.utilization_x1000 = candidate.utilization_x1000;
        reduced.util_updated_unix_ms = candidate.util_updated_unix_ms;
    }
    reduced
}

/// Per-provider query cache so admission/retry hot paths never stat+parse on
/// every call. A write updates the entry too, so this process sees its own
/// shared write immediately; other processes pick it up within the TTL.
static READ_CACHE: Mutex<[Option<(Instant, SharedQuota)>; crate::quota::PROVIDER_SLOTS]> =
    Mutex::new([None; crate::quota::PROVIDER_SLOTS]);

/// Per-provider timestamp (unix ms) of this process's last utilization write,
/// enforcing [`UTIL_WRITE_MIN_INTERVAL_MS`] so a busy response stream cannot
/// turn the shared file into a write hot spot.
static UTIL_LAST_WRITE_MS: [AtomicU64; crate::quota::PROVIDER_SLOTS] =
    [const { AtomicU64::new(0) }; crate::quota::PROVIDER_SLOTS];

fn cache_store(slot: usize, quota: SharedQuota) {
    lock_recovered(&READ_CACHE)[slot] = Some((Instant::now(), quota));
}

fn cached_read(kind: ProviderKind) -> SharedQuota {
    let slot = crate::quota::provider_slot(kind);
    {
        let cache = lock_recovered(&READ_CACHE);
        if let Some((at, quota)) = cache[slot] {
            if at.elapsed() < Duration::from_millis(READ_CACHE_TTL_MS) {
                return quota;
            }
        }
    }
    let quota = read_from_disk(kind);
    cache_store(slot, quota);
    quota
}

/// Drop all cached reads and write-throttle state. Test-only: the query cache is
/// a process-global keyed by provider slot, not by config home, so a test that
/// swaps `ZO_CONFIG_HOME` must clear it or it would read the previous home's
/// entry (in production the home never changes mid-process).
#[cfg(test)]
fn reset_for_test() {
    *lock_recovered(&READ_CACHE) = [None; crate::quota::PROVIDER_SLOTS];
    for slot in &UTIL_LAST_WRITE_MS {
        slot.store(0, Ordering::SeqCst);
    }
}

/// Record a 429 cool-down for the account: ratchet the shared deadline forward
/// and stamp the last-429 time. Called from `mark_rate_limit_cooldown`.
pub(crate) fn record_cooldown(kind: ProviderKind, until_unix_ms: u64) {
    if !active(kind) {
        return;
    }
    let path = shared_path(kind);
    let now = now_unix_millis();
    let capped_until = until_unix_ms.min(now.saturating_add(SHARED_COOLDOWN_MAX_MS));
    let mut merged = read_from_disk(kind);
    merged.cooldown_until_unix_ms = merged.cooldown_until_unix_ms.max(capped_until);
    merged.last_rate_limit_unix_ms = merged.last_rate_limit_unix_ms.max(now);
    atomic_write(&path, &merged.serialize());
    cache_store(crate::quota::provider_slot(kind), merged);
}

/// Utilization fraction → per-mille, clamped to `[0, 10_000]`. The clamp makes
/// the final narrowing provably lossless and non-negative (a hostile > 1000%
/// reading is capped at the same ceiling), so no truncation/sign surprise.
fn utilization_to_permille(utilization: f64) -> u64 {
    let permille = (utilization.clamp(0.0, 10.0) * 1000.0).floor();
    // Floor instead of round so the shared integer representation preserves the
    // exact threshold contract: every value below 0.95 remains below 950.
    // `permille` ∈ [0.0, 10_000.0] and integral after `floor`, so the cast is
    // exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let value = permille as u64;
    value
}

/// Record the account's current representative-window utilization as a proactive
/// leading signal. Throttled per process; cool-down fields preserved.
pub(crate) fn record_utilization(kind: ProviderKind, utilization: f64) {
    if !active(kind) {
        return;
    }
    let utilization_x1000 = utilization_to_permille(utilization);
    let slot = crate::quota::provider_slot(kind);
    let now = now_unix_millis();
    let last = UTIL_LAST_WRITE_MS[slot].load(Ordering::Relaxed);
    if now.saturating_sub(last) < UTIL_WRITE_MIN_INTERVAL_MS {
        return;
    }
    // Claim the write window first so concurrent callers in THIS process don't
    // all write; a lost claim just skips this update (the next one lands).
    if UTIL_LAST_WRITE_MS[slot]
        .compare_exchange(last, now, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let path = shared_path(kind);
    let mut merged = read_from_disk(kind);
    merged.utilization_x1000 = utilization_x1000;
    merged.util_updated_unix_ms = now;
    atomic_write(&path, &merged.serialize());
    cache_store(slot, merged);
}

/// Remaining shared cool-down for `kind` in ms (`0` if none / inactive), as
/// `deadline_unix - now_unix`. `max`'d with the in-process monotonic window by
/// the caller so whichever is longer wins.
#[must_use]
pub(crate) fn cooldown_remaining_ms(kind: ProviderKind) -> u64 {
    if !active(kind) {
        return 0;
    }
    cached_read(kind)
        .cooldown_until_unix_ms
        .saturating_sub(now_unix_millis())
}

/// Latest shared representative-window utilization and its age. Freshness is
/// decided by the caller so fallback and admission policies can share the same
/// reading without conflating it with recent-429 or cool-down signals.
pub(crate) fn latest_utilization(kind: ProviderKind) -> Option<(u64, Duration)> {
    if !active(kind) {
        return None;
    }
    let quota = cached_read(kind);
    (quota.util_updated_unix_ms > 0).then(|| {
        (
            quota.utilization_x1000,
            Duration::from_millis(
                now_unix_millis().saturating_sub(quota.util_updated_unix_ms),
            ),
        )
    })
}

/// `true` when the shared account signal says headroom is low: an active shared
/// cool-down, a shared 429 within [`SHARED_RECENT_429_WINDOW_MS`], or a fresh
/// shared utilization at/above [`SHARED_UTIL_THRESHOLD_X1000`]. `OR`'d into the
/// in-process headroom gate so any process's pressure tightens every governor.
#[must_use]
pub(crate) fn headroom_low(kind: ProviderKind) -> bool {
    if !active(kind) {
        return false;
    }
    let quota = cached_read(kind);
    let now = now_unix_millis();
    let cooldown_active = quota.cooldown_until_unix_ms > now;
    let recently_throttled = quota.last_rate_limit_unix_ms > 0
        && now.saturating_sub(quota.last_rate_limit_unix_ms) < SHARED_RECENT_429_WINDOW_MS;
    let util_hot = quota.utilization_x1000 >= SHARED_UTIL_THRESHOLD_X1000
        && now.saturating_sub(quota.util_updated_unix_ms) <= SHARED_UTIL_FRESH_MS;
    cooldown_active || recently_throttled || util_hot
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Isolate the shared file under a temp `ZO_CONFIG_HOME` and clear the
    /// query cache staleness by using distinct providers where possible. Tests
    /// mutate process-wide env, so they must not run under `cargo test`'s
    /// default parallelism without the shared env lock.
    fn with_temp_home<T>(body: impl FnOnce() -> T) -> T {
        let _lock = crate::test_env_lock();
        let dir = std::env::temp_dir().join(format!(
            "zo-shared-rate-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        let prior = [
            (
                core_types::paths::ZO_CONFIG_HOME_ENV,
                std::env::var_os(core_types::paths::ZO_CONFIG_HOME_ENV),
            ),
            (
                core_types::paths::ZO_HOME_ENV,
                std::env::var_os(core_types::paths::ZO_HOME_ENV),
            ),
            ("HOME", std::env::var_os("HOME")),
            (ENABLE_ENV, std::env::var_os(ENABLE_ENV)),
        ];
        std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, dir.join("config"));
        std::env::set_var(core_types::paths::ZO_HOME_ENV, dir.join("zo-home"));
        std::env::set_var("HOME", dir.join("user-home"));
        // Coordination is default-off under `cfg(test)` (see `enabled`), so opt
        // in explicitly only after every canonical read root is isolated.
        std::env::set_var(ENABLE_ENV, "1");
        // The query cache is process-global; clear any prior test's entry that
        // would otherwise shadow this fresh temp home.
        reset_for_test();
        let out = body();
        for (key, value) in prior {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    fn quota_path(root: &Path, kind: ProviderKind) -> PathBuf {
        root.join("rate")
            .join(shared_path(kind).file_name().expect("shared path has a file name"))
    }

    fn write_quota(root: &Path, kind: ProviderKind, quota: SharedQuota) {
        let path = quota_path(root, kind);
        std::fs::create_dir_all(path.parent().expect("quota path has a parent"))
            .expect("create quota directory");
        std::fs::write(path, quota.serialize()).expect("write quota record");
    }

    #[test]
    fn serialize_round_trips_through_parse() {
        let quota = SharedQuota {
            cooldown_until_unix_ms: 111,
            last_rate_limit_unix_ms: 222,
            utilization_x1000: 860,
            util_updated_unix_ms: 333,
        };
        assert_eq!(SharedQuota::parse(&quota.serialize()), Some(quota));
        assert_eq!(SharedQuota::parse("garbage"), None);
        assert_eq!(SharedQuota::parse("v2 1 2 3 4"), None);
        assert_eq!(SharedQuota::parse("v1 1 2 3"), None, "truncated record rejected");
        assert_eq!(SharedQuota::parse("v1 1 2 3 4 5"), None, "trailing field rejected");
    }

    #[test]
    fn cooldown_is_shared_and_ratchets_forward() {
        with_temp_home(|| {
            let kind = ProviderKind::Anthropic;
            assert_eq!(cooldown_remaining_ms(kind), 0, "no file → no cooldown");
            let now = now_unix_millis();
            record_cooldown(kind, now + 30_000);
            // A fresh reader (cache TTL is 1s, but a brand-new provider slot has
            // no cached entry yet) sees a live shared cool-down.
            assert!(cooldown_remaining_ms(kind) > 25_000, "shared cooldown visible");
            // A shorter mark must NOT shorten the live window.
            record_cooldown(kind, now + 5_000);
            assert!(
                cooldown_remaining_ms(kind) > 25_000,
                "cooldown only ratchets forward"
            );
        });
    }

    #[test]
    fn cooldown_is_capped_to_the_blast_radius_bound() {
        with_temp_home(|| {
            let kind = ProviderKind::Anthropic;
            record_cooldown(kind, now_unix_millis() + 10 * SHARED_COOLDOWN_MAX_MS);
            assert!(
                cooldown_remaining_ms(kind) <= SHARED_COOLDOWN_MAX_MS,
                "a hostile/huge deadline is capped so a racy write self-heals"
            );
        });
    }

    /// The harness detector is what closes the leak class: a downstream crate's
    /// test binary gets cross-process coordination OFF with no per-test opt-in,
    /// so a test that does not know it touches this state cannot park the
    /// developer's real account. `cargo test` runs every target from
    /// `target/<profile>/deps/`, which is what this reads.
    #[test]
    fn a_test_binary_is_detected_as_a_harness() {
        assert!(
            super::running_under_test_harness(),
            "this assertion runs inside a cargo test binary: {:?}",
            std::env::current_exe().ok()
        );
        // Belt and braces: with no explicit override, coordination is therefore
        // disabled here — the state a leaking test would otherwise write into.
        let _ = super::enabled();
    }

    #[test]
    fn utilization_threshold_is_95_percent() {
        with_temp_home(|| {
            let kind = ProviderKind::Anthropic;
            assert!(!headroom_low(kind), "empty shared state → healthy");
            record_utilization(kind, 0.9499);
            assert!(
                !headroom_low(kind),
                "fresh utilization below 95% must keep headroom available"
            );
            assert_eq!(
                utilization_to_permille(0.9499),
                949,
                "shared quantization must not round a below-threshold reading up"
            );
            reset_for_test();
            record_utilization(kind, 0.95);
            assert!(
                headroom_low(kind),
                "fresh utilization at 95% → low headroom"
            );
            assert_eq!(utilization_to_permille(0.95), 950);
            assert_eq!(utilization_to_permille(-1.0), 0, "negative clamps to 0");
            assert_eq!(
                utilization_to_permille(99.0),
                10_000,
                "hostile reading capped"
            );
        });
    }

    #[test]
    fn canonical_roots_reduce_newer_lower_signals_and_write_only_primary() {
        with_temp_home(|| {
            let kind = ProviderKind::Anthropic;
            let roots = core_types::paths::zo_global_config_roots();
            let primary = &roots[0];
            let lower = &roots[1];
            let malformed = &roots[2];
            let now = now_unix_millis();
            let lower_record = SharedQuota {
                cooldown_until_unix_ms: now + 60_000,
                last_rate_limit_unix_ms: now - 100,
                utilization_x1000: 975,
                util_updated_unix_ms: now - 100,
            };
            write_quota(
                primary,
                kind,
                SharedQuota {
                    cooldown_until_unix_ms: now + 1_000,
                    last_rate_limit_unix_ms: now - 10_000,
                    utilization_x1000: 300,
                    util_updated_unix_ms: now - 10_000,
                },
            );
            write_quota(lower, kind, lower_record);
            let malformed_path = quota_path(malformed, kind);
            std::fs::create_dir_all(malformed_path.parent().expect("quota path has a parent"))
                .expect("create malformed quota directory");
            std::fs::write(&malformed_path, "not a quota record\n")
                .expect("write malformed quota record");

            let reduced = read_from_disk(kind);
            assert_eq!(reduced.cooldown_until_unix_ms, lower_record.cooldown_until_unix_ms);
            assert_eq!(reduced.last_rate_limit_unix_ms, lower_record.last_rate_limit_unix_ms);
            assert_eq!(
                (reduced.utilization_x1000, reduced.util_updated_unix_ms),
                (lower_record.utilization_x1000, lower_record.util_updated_unix_ms),
                "newest utilization keeps its matching timestamp"
            );
            assert!(cooldown_remaining_ms(kind) > 50_000, "lower-root 429 is honored");
            assert_eq!(
                latest_utilization(kind).map(|(utilization, _)| utilization),
                Some(975),
                "newer lower-root utilization is honored"
            );

            record_utilization(kind, 0.5);
            let primary_after = SharedQuota::parse(
                &std::fs::read_to_string(quota_path(primary, kind))
                    .expect("read primary quota record"),
            )
            .expect("primary record remains valid");
            assert_eq!(
                primary_after.cooldown_until_unix_ms, lower_record.cooldown_until_unix_ms,
                "writers retain the reduced cooldown"
            );
            assert!(
                primary_after.last_rate_limit_unix_ms >= lower_record.last_rate_limit_unix_ms,
                "writers retain the reduced 429 timestamp"
            );
            assert_eq!(primary_after.utilization_x1000, 500);
            assert_eq!(
                std::fs::read_to_string(quota_path(lower, kind)).expect("read lower quota record"),
                lower_record.serialize(),
                "writers publish only to the primary root"
            );
            assert_eq!(
                std::fs::read_to_string(&malformed_path).expect("read malformed quota record"),
                "not a quota record\n",
                "malformed lower records are ignored, never overwritten"
            );
        });
    }

    #[test]
    fn disabled_env_makes_every_query_inert() {
        with_temp_home(|| {
            let kind = ProviderKind::Anthropic;
            std::env::set_var(ENABLE_ENV, "off");
            record_cooldown(kind, now_unix_millis() + 60_000);
            assert_eq!(cooldown_remaining_ms(kind), 0, "off: no shared cooldown");
            assert!(!headroom_low(kind), "off: no shared headroom signal");
            std::env::remove_var(ENABLE_ENV);
        });
    }

    #[test]
    fn non_coordinated_provider_is_left_to_its_own_governor() {
        with_temp_home(|| {
            // Even with a (hypothetical) shared file, a non-account-shared
            // provider never reads or writes it. Ollama is the clearest case:
            // it is self-hosted, so one deployment's window says nothing about
            // another's. (OpenAI used to stand in here and is now coordinated —
            // it reuses one credential and serves the cross-model VERIFY leg.)
            let kind = ProviderKind::Ollama;
            record_cooldown(kind, now_unix_millis() + 60_000);
            assert_eq!(cooldown_remaining_ms(kind), 0);
            assert!(!headroom_low(kind));
        });
    }
}
