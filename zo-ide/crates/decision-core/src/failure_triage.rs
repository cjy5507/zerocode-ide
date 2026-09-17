//! Escalation triage for long-horizon loop failures.
//!
//! The stall detector ([`ProgressTracker`](crate::loop_progress::ProgressTracker))
//! answers "did the *same* failure repeat?" by hashing the failure set. That
//! provably misses the worst runaway: a goal that is **impossible as scoped**
//! (a missing credential, a permission the agent cannot grant itself, an
//! unreachable service). Such a loop *re-plans* each turn, so its surface
//! failure text keeps changing and the identical-failure hash never matches —
//! it grinds the whole turn budget before a blunt wall-clock cap finally stops
//! it, burning quota the entire time.
//!
//! This module classifies *why* a turn failed into a coarse triage class whose
//! value is stable even when the surface text drifts: a loop that hits an
//! external blocker on turn 1 and a differently-worded external blocker on turn
//! 2 is still, both turns, `Blocked`. A short consecutive-`Blocked` streak
//! ([`BlockTracker`]) is therefore a signal the hash-based stall cannot produce,
//! and it lets the caller **escalate to the human with the specific blocker**
//! instead of retrying something that retrying cannot fix.
//!
//! Deliberately conservative and deterministic: it only reports `Blocked` on
//! strong, unambiguous external-block markers (auth/permission/missing-tool/
//! unreachable-host/failing-service), so an ordinary fixable compile or test
//! error stays `Hard` and the loop keeps working exactly as before. A future
//! model-judged layer can add an `Ambiguous` (spec under-specified) class; the
//! deterministic scan never fabricates one from text.
//!
//! Pure and total (no IO). Named `FailureTriage`/`BlockedNeed` to stay distinct
//! from the benchmark-lane [`FailureClass`](crate::decision::FailureClass),
//! which is a post-hoc run taxonomy, not a live escalation decision.

use serde::{Deserialize, Serialize};

/// Default consecutive-`Blocked` turns before the caller should escalate. Two
/// (not one) so a single fluke never escalates a legitimately hard goal: the
/// loop still gets one retry, and only a *persistent* external block escalates.
pub const BLOCK_ESCALATION_THRESHOLD: u32 = 2;

/// Coarse triage of one failing turn — the escalation decision, not a diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureTriage {
    /// A transient condition (rate limit, "try again later", temporarily
    /// unavailable). Retrying, unchanged, may well succeed — do not escalate.
    Transient,
    /// The loop cannot proceed without something outside its control: a
    /// credential, a permission, a required tool, or a reachable/healthy
    /// external service. Retrying cannot resolve it — escalate to the human.
    Blocked(BlockedNeed),
    /// Genuinely hard but plausibly achievable by the agent (a compile error, a
    /// failing assertion, a missing grep hit). Keep working — the default, and
    /// what every failure was treated as before this module existed.
    Hard,
}

/// What an external block needs from the human, driving the escalation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockedNeed {
    /// A filesystem/OS permission the agent cannot grant itself.
    Permission,
    /// An authentication credential (login, API key, token, SSH key).
    Credential,
    /// A host that is unreachable from this environment (DNS/refused/no route).
    Network,
    /// A required binary/tool that is not installed or not on `PATH`.
    MissingTool,
    /// An external service that is present but failing (TLS/cert, 5xx, down).
    ExternalService,
}

impl BlockedNeed {
    /// Short human label for the escalation digest.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Permission => "a filesystem/OS permission",
            Self::Credential => "an authentication credential",
            Self::Network => "network access to an unreachable host",
            Self::MissingTool => "a required tool that is not installed",
            Self::ExternalService => "a failing external service",
        }
    }

    /// The concrete action the human can take to unblock, appended to the
    /// escalation so the loop stops with a next step, not just a dead end.
    #[must_use]
    pub const fn remedy(self) -> &'static str {
        match self {
            Self::Permission => {
                "grant the needed file/exec permission (or point the goal at a writable path), then resume"
            }
            Self::Credential => {
                "provide the missing credential (log in, or set the API key/token), then resume"
            }
            Self::Network => {
                "the target host is unreachable from here — restore connectivity or fix the host, then resume"
            }
            Self::MissingTool => "install the missing tool so it is on PATH, then resume",
            Self::ExternalService => {
                "the external service is failing (TLS/cert or 5xx) — verify it is reachable and healthy, then resume"
            }
        }
    }
}

/// Classify a turn's *objective* validator failures into a triage class.
///
/// `Blocked` dominates: if any single failure is an external block, the turn is
/// `Blocked` even when another failure looks fixable — because the agent cannot
/// clear the block by working on the fixable part. Within `Blocked`, needs are
/// probed in a fixed priority so the most actionable one wins (a credential over
/// a bare permission, since "permission denied (publickey)" is really auth). An
/// empty failure set is `Hard` (no signal to escalate on).
#[must_use]
pub fn triage_failures(failures: &[String]) -> FailureTriage {
    // Marker tables, declared before any statement (items-after-statements).
    // Priority order: the most specific / most actionable need first. Each need
    // owns a set of unambiguous markers; the first need with any match wins.
    const CREDENTIAL: &[&str] = &[
        "401 unauthorized",
        "403 forbidden",
        "authentication required",
        "authentication failed",
        "could not read username",
        "could not read password",
        "invalid api key",
        "missing api key",
        "not logged in",
        "unauthenticated",
        "permission denied (publickey",
        "(publickey)",
        "host key verification failed",
    ];
    const PERMISSION: &[&str] = &[
        "permission denied",
        "eacces",
        "operation not permitted",
        "read-only file system",
        "readonly file system",
    ];
    const MISSING_TOOL: &[&str] = &[
        "command not found",
        "no such file or directory (os error 2)",
        "(os error 2)",
        "executable file not found",
        "is not recognized as an internal or external command",
        "cannot run program",
    ];
    const NETWORK: &[&str] = &[
        "could not resolve host",
        "name or service not known",
        "temporary failure in name resolution",
        "connection refused",
        "no route to host",
        "network is unreachable",
        "failed to lookup address information",
        "dns error",
    ];
    const EXTERNAL_SERVICE: &[&str] = &[
        "ssl certificate problem",
        "certificate verify failed",
        "certificate has expired",
        "tls handshake",
        "handshake failed",
        "502 bad gateway",
        "503 service unavailable",
        "500 internal server error",
        "service unavailable",
    ];
    // Transient markers escalate to nothing — a retry is the right move. Checked
    // only after every Blocked category, so "temporary failure in name
    // resolution" (a DNS block) is caught by NETWORK before "temporary" here.
    const TRANSIENT: &[&str] = &[
        "429",
        "too many requests",
        "rate limit",
        "rate-limited",
        "try again later",
        "temporarily unavailable",
    ];

    let haystack = failures
        .iter()
        .map(|failure| failure.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    if haystack.trim().is_empty() {
        return FailureTriage::Hard;
    }

    let matches_any = |markers: &[&str]| markers.iter().any(|m| haystack.contains(m));

    for (need, markers) in [
        (BlockedNeed::Credential, CREDENTIAL),
        (BlockedNeed::Permission, PERMISSION),
        (BlockedNeed::MissingTool, MISSING_TOOL),
        (BlockedNeed::Network, NETWORK),
        (BlockedNeed::ExternalService, EXTERNAL_SERVICE),
    ] {
        if matches_any(markers) {
            return FailureTriage::Blocked(need);
        }
    }
    if matches_any(TRANSIENT) {
        return FailureTriage::Transient;
    }
    FailureTriage::Hard
}

/// Consecutive-`Blocked` streak tracker. Serializable so a resumed loop keeps
/// its streak, mirroring [`ProgressTracker`](crate::loop_progress::ProgressTracker).
///
/// Unlike the stall tracker it keys on the triage *class*, not a failure hash:
/// that is exactly what lets it fire when an impossible-as-scoped goal re-plans
/// each turn with different-looking (but still externally blocked) failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BlockTracker {
    consecutive_blocked: u32,
}

impl BlockTracker {
    /// Fold one *failing* turn's triage into the streak. A `Blocked` turn
    /// extends the streak and, once it reaches `threshold` (`> 0`), returns the
    /// need to escalate on; any non-`Blocked` triage resets the streak. Call on
    /// every failing turn so the streak reflects genuine consecutiveness.
    pub fn observe(&mut self, triage: FailureTriage, threshold: u32) -> Option<BlockedNeed> {
        match triage {
            FailureTriage::Blocked(need) => {
                self.consecutive_blocked = self.consecutive_blocked.saturating_add(1);
                (threshold > 0 && self.consecutive_blocked >= threshold).then_some(need)
            }
            FailureTriage::Transient | FailureTriage::Hard => {
                self.consecutive_blocked = 0;
                None
            }
        }
    }

    /// Current consecutive-`Blocked` count (for diagnostics/tests).
    #[must_use]
    pub const fn streak(self) -> u32 {
        self.consecutive_blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> Vec<String> {
        vec![text.to_string()]
    }

    #[test]
    fn empty_and_plain_errors_are_hard() {
        assert_eq!(triage_failures(&[]), FailureTriage::Hard);
        assert_eq!(triage_failures(&s("   ")), FailureTriage::Hard);
        // A fixable compile error must stay Hard — the agent can resolve it.
        assert_eq!(
            triage_failures(&s(
                "cargo:check failed (exit 101, timed_out=false): error[E0432]: unresolved import `foo::bar`"
            )),
            FailureTriage::Hard
        );
        // A failing assertion / missing grep hit is Hard, not blocked.
        assert_eq!(
            triage_failures(&s("grep:TODO not found in workspace text files")),
            FailureTriage::Hard
        );
    }

    #[test]
    fn permission_denied_is_blocked_permission() {
        assert_eq!(
            triage_failures(&s(
                "cargo:test failed (exit 101, timed_out=false): error: Permission denied (os error 13)"
            )),
            FailureTriage::Blocked(BlockedNeed::Permission)
        );
        assert_eq!(
            triage_failures(&s("git diff --check failed (exit 128): error: Read-only file system")),
            FailureTriage::Blocked(BlockedNeed::Permission)
        );
    }

    #[test]
    fn ssh_publickey_is_credential_not_bare_permission() {
        // Contains "permission denied" AND "(publickey)": credential wins, since
        // it is checked first and is the actionable cause.
        assert_eq!(
            triage_failures(&s("fatal: Permission denied (publickey).")),
            FailureTriage::Blocked(BlockedNeed::Credential)
        );
    }

    #[test]
    fn auth_markers_are_credential() {
        for text in [
            "remote: HTTP 401 Unauthorized",
            "error: authentication required",
            "fatal: could not read Username for 'https://github.com'",
            "error: invalid api key provided",
        ] {
            assert_eq!(
                triage_failures(&s(text)),
                FailureTriage::Blocked(BlockedNeed::Credential),
                "{text}"
            );
        }
    }

    #[test]
    fn missing_tool_markers() {
        for text in [
            "error: No such file or directory (os error 2)",
            "sh: cargo-nextest: command not found",
            "cannot run program \"pnpm\"",
        ] {
            assert_eq!(
                triage_failures(&s(text)),
                FailureTriage::Blocked(BlockedNeed::MissingTool),
                "{text}"
            );
        }
    }

    #[test]
    fn network_markers_including_temporary_name_resolution() {
        for text in [
            "error: could not resolve host: github.com",
            "curl: (7) Connection refused",
            "Temporary failure in name resolution",
            "network is unreachable",
        ] {
            assert_eq!(
                triage_failures(&s(text)),
                FailureTriage::Blocked(BlockedNeed::Network),
                "{text}"
            );
        }
    }

    #[test]
    fn external_service_and_tls_markers() {
        for text in [
            "curl: (60) SSL certificate problem: unable to get local issuer",
            "error: TLS handshake failed",
            "server returned 503 Service Unavailable",
        ] {
            assert_eq!(
                triage_failures(&s(text)),
                FailureTriage::Blocked(BlockedNeed::ExternalService),
                "{text}"
            );
        }
    }

    #[test]
    fn rate_limit_is_transient_not_blocked() {
        assert_eq!(
            triage_failures(&s("error: HTTP 429 Too Many Requests, rate limit exceeded")),
            FailureTriage::Transient
        );
    }

    #[test]
    fn blocked_dominates_a_mixed_failure_set() {
        // One fixable error + one hard external block ⇒ the turn is Blocked,
        // because the agent cannot clear the block by fixing the other error.
        let failures = vec![
            "cargo:check failed: error[E0308]: mismatched types".to_string(),
            "cargo:test failed: error: Permission denied".to_string(),
        ];
        assert_eq!(
            triage_failures(&failures),
            FailureTriage::Blocked(BlockedNeed::Permission)
        );
    }

    #[test]
    fn tracker_escalates_only_on_consecutive_blocked() {
        let mut tracker = BlockTracker::default();
        let blocked = FailureTriage::Blocked(BlockedNeed::Credential);
        // First blocked turn: streak 1 < 2 ⇒ no escalation (one retry allowed).
        assert_eq!(tracker.observe(blocked, BLOCK_ESCALATION_THRESHOLD), None);
        assert_eq!(tracker.streak(), 1);
        // Second consecutive: escalate with the need.
        assert_eq!(
            tracker.observe(blocked, BLOCK_ESCALATION_THRESHOLD),
            Some(BlockedNeed::Credential)
        );
    }

    #[test]
    fn tracker_reports_latest_need_when_class_stays_blocked() {
        // Surface text drifts (the "re-planning around an impossible goal" case):
        // the need differs turn to turn but the class stays Blocked, so the
        // streak survives and escalates with the most recent need.
        let mut tracker = BlockTracker::default();
        assert_eq!(
            tracker.observe(FailureTriage::Blocked(BlockedNeed::Network), 2),
            None
        );
        assert_eq!(
            tracker.observe(FailureTriage::Blocked(BlockedNeed::ExternalService), 2),
            Some(BlockedNeed::ExternalService),
            "different blocked need still consecutive ⇒ escalates"
        );
    }

    #[test]
    fn non_blocked_turn_resets_the_streak() {
        let mut tracker = BlockTracker::default();
        assert_eq!(
            tracker.observe(FailureTriage::Blocked(BlockedNeed::Permission), 2),
            None
        );
        // A Hard (or Transient) turn means progress is still possible: reset.
        assert_eq!(tracker.observe(FailureTriage::Hard, 2), None);
        assert_eq!(tracker.streak(), 0);
        assert_eq!(
            tracker.observe(FailureTriage::Blocked(BlockedNeed::Permission), 2),
            None,
            "streak restarted from zero"
        );
    }

    #[test]
    fn threshold_zero_never_escalates() {
        let mut tracker = BlockTracker::default();
        for _ in 0..10 {
            assert_eq!(
                tracker.observe(FailureTriage::Blocked(BlockedNeed::Permission), 0),
                None
            );
        }
    }

    #[test]
    fn tracker_roundtrips_through_serde() {
        let mut tracker = BlockTracker::default();
        tracker.observe(FailureTriage::Blocked(BlockedNeed::Network), 5);
        let json = serde_json::to_string(&tracker).expect("serialize");
        let back: BlockTracker = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tracker, back);
    }
}
