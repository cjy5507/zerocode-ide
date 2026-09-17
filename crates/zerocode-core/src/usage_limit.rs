//! When a usage read fails, WHAT it failed of — and what that answer earns.
//!
//! Measured from Orca 1.4.178-rc.2. The window shows a plan's remaining quota
//! in the status bar, and every road to that number can fail: no token on
//! file, a token the server no longer accepts, a 429, a name that will not
//! resolve. Our first cut answered all of them with a free-form sentence, and
//! a sentence is a thing you can print but cannot decide with. Three decisions
//! hang off knowing WHICH failure it was, and none of them can be made from
//! prose:
//!
//! - **How long a stale number keeps standing.** A rate-limited read lets the
//!   last good snapshot stand for a whole day; every other failure gives it
//!   half an hour ([`stale_threshold_ms`], `service.ts:2121-2125`). Without the
//!   distinction the bar either flaps to empty on one dropped packet or shows
//!   a day-old number after a parse error.
//! - **When to ask again.** A server that said `Retry-After` has told us, and
//!   asking earlier keeps its 429 alive ([`retry_after_ms`]). A failure with no
//!   such answer backs off by doubling instead ([`failure_backoff_ms`]).
//! - **Whether the terminal road is worth walking.** An auth or limit answer
//!   from the API IS the answer; falling through to the CLI spawns an agent
//!   process for nothing ([`skips_pty_fallback`], `claude-oauth-usage-error.ts:19-21`).
//!
//! The DECISIONS live here, pure and testable; the shell owns the clock, the
//! socket, and the calendar — same split as [`crate::notify`]. In particular
//! this module never parses a date: `Retry-After` may carry an HTTP-date, and
//! the caller hands in its own reader for that (see [`retry_after_ms`]) rather
//! than a fifth copy of the civil-date arithmetic already in the tree.

use serde::{Deserialize, Serialize};

/// What a usage read failed of.
///
/// Orca's own fourteen, spelled its way on the wire so a snapshot written by
/// either program reads in the other (`shared/rate-limit-types.ts:20-34`).
/// Kept whole rather than trimmed to the ones our current roads produce: the
/// set is a vocabulary, and a provider added later must not have to widen it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    /// Nothing on file to authenticate with.
    MissingCredentials,
    /// A token the server no longer accepts.
    StaleToken,
    /// Something refreshable is on file, but no usable token came out of it.
    RefreshableCredentialsWithoutToken,
    /// The refresh belongs to somebody else's process.
    DelegatedRefreshRequired,
    /// A live session holds the credential; refreshing under it would fight.
    DeferredByLiveSession,
    /// The OS keychain refused or was not there.
    KeychainUnavailable,
    /// Authenticated, but the token lacks the scope the endpoint wants.
    MissingScope,
    /// The request never reached a server.
    Network,
    /// The server answered, and the answer was its own failure.
    Server,
    /// The body arrived and was not what it claimed to be.
    Parse,
    /// The server asked us to stop for a while.
    RateLimited,
    /// The vendor's command-line road is not available to walk.
    CliUnavailable,
    /// The account has no plan figures to report.
    UsageUnavailable,
    /// None of the above, which is a real answer and not a placeholder.
    Unknown,
}

/// A failure, and the two roads still open after it.
///
/// Orca ships four fields but only ever builds three of the eight
/// combinations (`claude-usage-error-classification.ts:59-84`), so the three
/// are constructors here and the illegal five cannot be spelled at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recovery {
    pub kind: FailureKind,
    /// Whether the vendor's CLI is still worth asking.
    pub cli_fallback: bool,
    /// Whether somebody else's process should be asked to refresh the token.
    pub delegated_refresh: bool,
    /// Whether this answer ends the attempt.
    pub terminal: bool,
}

impl Recovery {
    /// The token can be renewed and the CLI can be asked — `recoverableAuth`
    /// (`claude-usage-error-classification.ts:59-66`).
    const fn recoverable_auth(kind: FailureKind) -> Self {
        Self {
            kind,
            cli_fallback: true,
            delegated_refresh: true,
            terminal: false,
        }
    }

    /// The CLI is worth asking; renewing a token would not help —
    /// `fallbackOnly` (`:68-75`).
    const fn fallback_only(kind: FailureKind) -> Self {
        Self {
            kind,
            cli_fallback: true,
            delegated_refresh: false,
            terminal: false,
        }
    }

    /// This is the answer — `terminal` (`:77-84`).
    const fn terminal(kind: FailureKind) -> Self {
        Self {
            kind,
            cli_fallback: false,
            delegated_refresh: false,
            terminal: true,
        }
    }
}

/// The substring that tells a scope refusal from an expired one.
///
/// A 403 means both things at this endpoint, and the only thing separating
/// them is the scope named in the body (`claude-usage-error-classification.ts:20-22`).
const MISSING_SCOPE_MARKER: &str = "user:profile";

/// What an HTTP answer failed of (`classifyClaudeOAuthUsageError`,
/// `claude-usage-error-classification.ts:13-27`).
///
/// `body` is the server's own message, consulted only to split the 403.
#[must_use]
pub fn classify_http(status: u16, body: &str) -> Recovery {
    match status {
        429 => Recovery::terminal(FailureKind::RateLimited),
        401 => Recovery::recoverable_auth(FailureKind::StaleToken),
        403 if body.contains(MISSING_SCOPE_MARKER) => Recovery::terminal(FailureKind::MissingScope),
        403 => Recovery::recoverable_auth(FailureKind::StaleToken),
        500..=599 => Recovery::fallback_only(FailureKind::Server),
        _ => Recovery::terminal(FailureKind::UsageUnavailable),
    }
}

/// The words a request that never got an answer uses about itself.
///
/// Orca tests one regex against the thrown message
/// (`claude-usage-error-classification.ts:35`); the same needles, matched
/// case-insensitively, because `reqwest`'s prose is not `fetch`'s and only the
/// vocabulary carries over.
const NETWORK_MARKERS: [&str; 7] = [
    "abort",
    "network",
    "econn",
    "enotfound",
    "etimedout",
    "fetch failed",
    "dns",
];

/// What a failure that never reached a server failed of
/// (`claude-usage-error-classification.ts:30-38`).
///
/// `body_was_unreadable` stands for Orca's `error instanceof SyntaxError`: the
/// answer arrived and would not parse, which is the server's problem and not
/// the wire's.
#[must_use]
pub fn classify_transport(message: &str, body_was_unreadable: bool) -> Recovery {
    if body_was_unreadable {
        return Recovery::fallback_only(FailureKind::Parse);
    }
    let said = message.to_ascii_lowercase();
    if NETWORK_MARKERS.iter().any(|needle| said.contains(needle)) {
        return Recovery::fallback_only(FailureKind::Network);
    }
    Recovery::fallback_only(FailureKind::Unknown)
}

/// What having nothing to authenticate with failed of
/// (`classifyClaudeCredentialAbsence`, `claude-usage-error-classification.ts:41-57`).
///
/// The order is the contract: a live session's claim on the credential beats
/// the fact that the credential is refreshable, because refreshing under a
/// running agent is what the deferral exists to prevent (pinned by Orca's own
/// test at `claude-usage-error-classification.test.ts:73-83`).
#[must_use]
pub const fn classify_absent_credentials(
    has_refreshable_credentials: bool,
    keychain_unavailable: bool,
    deferred_by_live_session: bool,
) -> Recovery {
    if deferred_by_live_session {
        Recovery::terminal(FailureKind::DeferredByLiveSession)
    } else if keychain_unavailable {
        Recovery::fallback_only(FailureKind::KeychainUnavailable)
    } else if has_refreshable_credentials {
        Recovery::recoverable_auth(FailureKind::RefreshableCredentialsWithoutToken)
    } else {
        Recovery::terminal(FailureKind::MissingCredentials)
    }
}

/// Whether the vendor's terminal road should be skipped after this status.
///
/// An auth or limit answer from the usage API is already the user-visible
/// answer; falling through spawns the agent's CLI for nothing
/// (`claude-oauth-usage-error.ts:19-21`).
#[must_use]
pub const fn skips_pty_fallback(status: u16) -> bool {
    matches!(status, 401 | 403 | 429)
}

/// The longest a `Retry-After` may hold refreshes off.
///
/// "a corrupt/hostile Retry-After must not gate usage refreshes for days"
/// (`claude-oauth-usage-error.ts:1-2`).
pub const MAX_RETRY_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

/// How long to wait before asking again, when the server said so.
///
/// Read ONLY on 429 (`claude-oauth-usage-error.ts:22`): a 401 carrying a
/// `Retry-After` is answering a different question, and Orca's own test pins
/// that it yields nothing (`claude-oauth-usage-error.test.ts:44-53`).
///
/// Delta-seconds first, then RFC 9110's HTTP-date. This module owns neither a
/// clock nor a calendar, so the caller passes `now_ms` and `http_date_ms` — the
/// latter being its own reader for a date string, which in this tree is the
/// `days_from_civil` arithmetic that already sits beside the usage roads.
/// Anything not positive is nothing, and everything is capped.
pub fn retry_after_ms(
    status: u16,
    header: Option<&str>,
    now_ms: i64,
    http_date_ms: impl FnOnce(&str) -> Option<i64>,
) -> Option<i64> {
    if status != 429 {
        return None;
    }
    let header = header?.trim();
    if header.is_empty() {
        return None;
    }
    // `Number(header)` in Orca: a bare count of seconds, and a fractional one
    // is still a number there, so it is one here.
    if let Ok(seconds) = header.parse::<f64>() {
        if !seconds.is_finite() || seconds <= 0.0 {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the value is clamped to a day in milliseconds long before it \
                      can reach the edge of an i64"
        )]
        return Some((seconds * 1000.0).min(MAX_RETRY_AFTER_MS as f64) as i64);
    }
    let delta = http_date_ms(header)?.checked_sub(now_ms)?;
    (delta > 0).then(|| delta.min(MAX_RETRY_AFTER_MS))
}

/// How long a good snapshot keeps standing after a failed read.
///
/// Thirty minutes for most things (`service.ts:90`); a whole day when the
/// failure was a limit, because "usage-endpoint 429 windows can outlast the
/// generic threshold (Retry-After ~1h); quota is informational, so a stale
/// snapshot beats a bare 'Limited'" (`service.ts:91-92`, chosen at `:2121-2125`).
pub const STALE_THRESHOLD_MS: i64 = 30 * 60 * 1000;

/// The same, for a read that failed because the server said to stop.
pub const RATE_LIMITED_STALE_THRESHOLD_MS: i64 = 24 * 60 * 60 * 1000;

/// Which of the two thresholds this failure earns.
#[must_use]
pub const fn stale_threshold_ms(kind: FailureKind) -> i64 {
    match kind {
        FailureKind::RateLimited => RATE_LIMITED_STALE_THRESHOLD_MS,
        _ => STALE_THRESHOLD_MS,
    }
}

/// Whether a snapshot taken at `previous_updated_at` may still stand after a
/// read that failed.
///
/// "keep showing a recent snapshot through repeated transient failures until
/// it ages out, so the bar doesn't flap to empty" (`service.ts:2141`). A read
/// that failed with no kind on it gets the ordinary threshold: the terminal
/// road produces those, and thirty minutes is what Orca gives anything that
/// is not a limit.
#[must_use]
pub const fn may_stand(kind: Option<FailureKind>, previous_updated_at: i64, now_ms: i64) -> bool {
    let threshold = match kind {
        Some(kind) => stale_threshold_ms(kind),
        None => STALE_THRESHOLD_MS,
    };
    now_ms - previous_updated_at <= threshold
}

/// The floor a failing provider is retried at (`service.ts:80`, itself
/// `MIN_POLL_MS` — "renderer input should never create a tight loop").
pub const ACTIVE_FAILURE_REFETCH_MS: i64 = 30 * 1000;

/// And the ceiling that doubling walks up to (`service.ts:82`, itself the
/// ordinary poll cadence): "retrying a persistent failure at the 30s floor
/// hammers endpoints into 429s; back off per failure, capped at the poll
/// cadence" (`service.ts:81`).
pub const MAX_ACTIVE_FAILURE_REFETCH_MS: i64 = 15 * 60 * 1000;

/// How far the streak itself is counted before it stops mattering
/// (`service.ts:83`).
pub const MAX_ACTIVE_FAILURE_STREAK: u32 = 8;

/// How long to wait before retrying a provider that has failed `streak` times
/// in a row, when the server named no time of its own.
///
/// `min(30s * 2^(streak-1), 15min)` — Orca's own arithmetic, including the
/// `max(0, …)` that makes the first failure wait the floor rather than half of
/// it (`service.ts:902-906`).
#[must_use]
pub const fn failure_backoff_ms(streak: u32) -> i64 {
    let steps = if streak > 0 { streak - 1 } else { 0 };
    // Saturating by construction: the cap is reached at nine doublings and the
    // shift would be undefined long before an i64 could overflow.
    if steps >= MAX_ACTIVE_FAILURE_STREAK {
        return MAX_ACTIVE_FAILURE_REFETCH_MS;
    }
    let waited = ACTIVE_FAILURE_REFETCH_MS << steps;
    if waited > MAX_ACTIVE_FAILURE_REFETCH_MS {
        MAX_ACTIVE_FAILURE_REFETCH_MS
    } else {
        waited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HTTP ladder, status by status — and the 403 that means two things.
    #[test]
    fn an_http_answer_is_classified_by_its_status_and_the_403_by_its_body() {
        assert_eq!(classify_http(429, "").kind, FailureKind::RateLimited);
        assert!(classify_http(429, "").terminal);
        assert!(!classify_http(429, "").cli_fallback);

        assert_eq!(classify_http(401, "").kind, FailureKind::StaleToken);
        assert!(classify_http(401, "").delegated_refresh);

        // The same status, two answers, told apart only by the scope the body
        // names.
        assert_eq!(
            classify_http(403, "missing scope: user:profile").kind,
            FailureKind::MissingScope
        );
        assert!(classify_http(403, "missing scope: user:profile").terminal);
        assert_eq!(classify_http(403, "expired").kind, FailureKind::StaleToken);
        assert!(!classify_http(403, "expired").terminal);

        for status in [500, 502, 503, 599] {
            assert_eq!(classify_http(status, "").kind, FailureKind::Server);
            assert!(classify_http(status, "").cli_fallback);
        }
        assert_eq!(classify_http(404, "").kind, FailureKind::UsageUnavailable);
        assert_eq!(classify_http(418, "").kind, FailureKind::UsageUnavailable);
    }

    /// A failure that never reached a server, and one whose body was rubbish.
    #[test]
    fn a_failure_short_of_the_server_is_told_from_one_that_reached_it() {
        assert_eq!(
            classify_transport("anything at all", true).kind,
            FailureKind::Parse
        );
        for said in [
            "error sending request: dns error",
            "operation was aborted",
            "ECONNREFUSED",
            "Network is unreachable",
            "ETIMEDOUT",
            "fetch failed",
            "ENOTFOUND api.anthropic.com",
        ] {
            assert_eq!(
                classify_transport(said, false).kind,
                FailureKind::Network,
                "{said} did not read as a network failure"
            );
        }
        // Unknown is an answer, not a placeholder — and it still lets the CLI
        // road be walked.
        let strange = classify_transport("the moon was in the way", false);
        assert_eq!(strange.kind, FailureKind::Unknown);
        assert!(strange.cli_fallback);
        assert!(!strange.terminal);
    }

    /// Absence has an order, and the live session is at the front of it.
    #[test]
    fn a_live_session_claims_the_credential_before_refreshability_is_asked() {
        // Both true: the deferral wins (Orca's own test pins this pair).
        assert_eq!(
            classify_absent_credentials(true, false, true).kind,
            FailureKind::DeferredByLiveSession
        );
        assert_eq!(
            classify_absent_credentials(true, true, false).kind,
            FailureKind::KeychainUnavailable
        );
        assert_eq!(
            classify_absent_credentials(true, false, false).kind,
            FailureKind::RefreshableCredentialsWithoutToken
        );
        assert_eq!(
            classify_absent_credentials(false, false, false).kind,
            FailureKind::MissingCredentials
        );
    }

    /// `Retry-After` is a 429's word and nobody else's.
    #[test]
    fn retry_after_is_read_only_on_a_limit_and_never_past_a_day() {
        let none = |_: &str| None;
        assert_eq!(retry_after_ms(429, Some("60"), 0, none), Some(60_000));
        // A 401 carrying the same header yields nothing: it is answering a
        // different question.
        assert_eq!(retry_after_ms(401, Some("60"), 0, none), None);
        assert_eq!(retry_after_ms(503, Some("60"), 0, none), None);

        // Nothing, nonsense, zero and negative are all nothing.
        assert_eq!(retry_after_ms(429, None, 0, none), None);
        assert_eq!(retry_after_ms(429, Some("   "), 0, none), None);
        assert_eq!(retry_after_ms(429, Some("0"), 0, none), None);
        assert_eq!(retry_after_ms(429, Some("-5"), 0, none), None);
        assert_eq!(retry_after_ms(429, Some("soon"), 0, none), None);

        // A hostile number is capped rather than believed.
        assert_eq!(
            retry_after_ms(429, Some("999999999"), 0, none),
            Some(MAX_RETRY_AFTER_MS)
        );

        // The date road: the caller's own reader, and the delta from now.
        let at = |_: &str| Some(1_000_000_i64);
        assert_eq!(
            retry_after_ms(429, Some("Sun, 06 Nov 1994 08:49:37 GMT"), 400_000, at),
            Some(600_000)
        );
        // A date already past is nothing, not a negative wait.
        assert_eq!(
            retry_after_ms(429, Some("Sun, 06 Nov 1994 08:49:37 GMT"), 2_000_000, at),
            None
        );
        // And a date beyond the cap is capped.
        assert_eq!(
            retry_after_ms(
                429,
                Some("Sun, 06 Nov 1994 08:49:37 GMT"),
                -MAX_RETRY_AFTER_MS,
                at
            ),
            Some(MAX_RETRY_AFTER_MS)
        );
    }

    /// A limit lets yesterday's number stand; everything else gets half an hour.
    #[test]
    fn only_a_limit_lets_a_snapshot_stand_for_a_day() {
        assert_eq!(
            stale_threshold_ms(FailureKind::RateLimited),
            RATE_LIMITED_STALE_THRESHOLD_MS
        );
        for kind in [
            FailureKind::Network,
            FailureKind::Server,
            FailureKind::Parse,
            FailureKind::StaleToken,
            FailureKind::Unknown,
        ] {
            assert_eq!(stale_threshold_ms(kind), STALE_THRESHOLD_MS);
        }
    }

    /// A recent snapshot outlives a failure; an old one does not.
    #[test]
    fn a_snapshot_stands_until_it_ages_out_of_its_kinds_window() {
        let now = 1_000_000_000_000;
        // Half an hour for an ordinary failure, to the millisecond.
        assert!(may_stand(
            Some(FailureKind::Network),
            now - STALE_THRESHOLD_MS,
            now
        ));
        assert!(!may_stand(
            Some(FailureKind::Network),
            now - STALE_THRESHOLD_MS - 1,
            now
        ));
        // A limit keeps yesterday's figure, which is the whole point of the
        // second threshold: a 429 window outlasts the generic one.
        assert!(may_stand(
            Some(FailureKind::RateLimited),
            now - STALE_THRESHOLD_MS - 1,
            now
        ));
        assert!(may_stand(
            Some(FailureKind::RateLimited),
            now - RATE_LIMITED_STALE_THRESHOLD_MS,
            now
        ));
        assert!(!may_stand(
            Some(FailureKind::RateLimited),
            now - RATE_LIMITED_STALE_THRESHOLD_MS - 1,
            now
        ));
        // A failure with no kind on it — the terminal road's — gets the
        // ordinary window rather than none at all.
        assert!(may_stand(None, now - STALE_THRESHOLD_MS, now));
        assert!(!may_stand(None, now - STALE_THRESHOLD_MS - 1, now));
    }

    /// The backoff doubles from the floor and stops at the poll cadence.
    #[test]
    fn a_repeated_failure_backs_off_by_doubling_and_stops_at_the_cadence() {
        // The first failure waits the floor, not half of it — the `max(0, …)`.
        assert_eq!(failure_backoff_ms(0), ACTIVE_FAILURE_REFETCH_MS);
        assert_eq!(failure_backoff_ms(1), 30_000);
        assert_eq!(failure_backoff_ms(2), 60_000);
        assert_eq!(failure_backoff_ms(3), 120_000);
        assert_eq!(failure_backoff_ms(4), 240_000);
        assert_eq!(failure_backoff_ms(5), 480_000);
        // 960_000 would pass the cadence, so the cadence is what it waits.
        assert_eq!(failure_backoff_ms(6), MAX_ACTIVE_FAILURE_REFETCH_MS);
        assert_eq!(failure_backoff_ms(50), MAX_ACTIVE_FAILURE_REFETCH_MS);
        assert_eq!(failure_backoff_ms(u32::MAX), MAX_ACTIVE_FAILURE_REFETCH_MS);
    }

    /// The terminal road is skipped exactly where the API already answered.
    #[test]
    fn an_auth_or_limit_answer_ends_the_attempt() {
        for status in [401, 403, 429] {
            assert!(skips_pty_fallback(status));
        }
        for status in [200, 404, 500, 503] {
            assert!(!skips_pty_fallback(status));
        }
    }

    /// The wire spelling is Orca's, or a snapshot written by one program is
    /// unreadable by the other.
    #[test]
    fn the_kinds_are_spelled_the_way_orca_spells_them() {
        let said = |kind: FailureKind| serde_json::to_string(&kind).expect("a kind refused JSON");
        assert_eq!(
            said(FailureKind::MissingCredentials),
            "\"missing-credentials\""
        );
        assert_eq!(said(FailureKind::StaleToken), "\"stale-token\"");
        assert_eq!(
            said(FailureKind::RefreshableCredentialsWithoutToken),
            "\"refreshable-credentials-without-token\""
        );
        assert_eq!(
            said(FailureKind::DelegatedRefreshRequired),
            "\"delegated-refresh-required\""
        );
        assert_eq!(
            said(FailureKind::DeferredByLiveSession),
            "\"deferred-by-live-session\""
        );
        assert_eq!(
            said(FailureKind::KeychainUnavailable),
            "\"keychain-unavailable\""
        );
        assert_eq!(said(FailureKind::MissingScope), "\"missing-scope\"");
        assert_eq!(said(FailureKind::Network), "\"network\"");
        assert_eq!(said(FailureKind::Server), "\"server\"");
        assert_eq!(said(FailureKind::Parse), "\"parse\"");
        assert_eq!(said(FailureKind::RateLimited), "\"rate-limited\"");
        assert_eq!(said(FailureKind::CliUnavailable), "\"cli-unavailable\"");
        assert_eq!(said(FailureKind::UsageUnavailable), "\"usage-unavailable\"");
        assert_eq!(said(FailureKind::Unknown), "\"unknown\"");
    }
}
