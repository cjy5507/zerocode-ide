//! Per-branch memory of failed OAuth refresh attempts.
//!
//! A rotating refresh token can be spent exactly once: the token endpoint hands
//! back a replacement and forgets its predecessor. Zo holds the Claude
//! subscription grant in two places — the Claude Code keychain and its own
//! credential file — so a branch that was superseded by whoever refreshed last
//! is still on disk, still looks like a credential, and can only ever answer
//! `invalid_grant`.
//!
//! Retrying such a branch is not a recoverable error, it is a guaranteed
//! round-trip to a 400 on the path that resolves credentials — once per turn.
//! The previous guard was a single process-wide timestamp, which could not tell
//! a revoked token from a flaky network and blocked *every* branch for a minute
//! whichever it was.
//!
//! So failures are recorded against a hash of the refresh token that failed
//! (never the token itself, which must not sit in process memory a second time):
//!
//! * a terminal rejection (`invalid_grant`) retires that branch for the rest of
//!   the process — nothing but a fresh sign-in can revive it, and a fresh
//!   sign-in mints a *different* token, so it is never held back by this;
//! * anything else (offline, timeout, 5xx) cools down briefly and is retried.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::ApiError;

/// How long a non-terminal refresh failure suppresses another attempt on the
/// same branch. The resolution path runs at every turn boundary while auth is in
/// fallback, so an unguarded failure hammers the token endpoint.
const TRANSIENT_COOLDOWN: Duration = Duration::from_secs(60);

/// Why an attempt is being held back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshBlock {
    /// The endpoint rejected this exact token; only a new sign-in helps.
    Retired,
    /// A transient failure is still cooling down.
    CoolingDown,
}

#[derive(Debug, Clone, Copy)]
enum Failure {
    Retired,
    Transient(Instant),
}

/// Poison policy: recover — the map is plain data and a panicking caller cannot
/// leave it torn.
static FAILURES: Mutex<Option<HashMap<u64, Failure>>> = Mutex::new(None);

fn fingerprint(refresh_token: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    refresh_token.hash(&mut hasher);
    hasher.finish()
}

fn with_failures<T>(f: impl FnOnce(&mut HashMap<u64, Failure>) -> T) -> T {
    let mut guard = FAILURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(guard.get_or_insert_with(HashMap::new))
}

/// `None` when this branch may be refreshed now.
pub(crate) fn refresh_blocked(refresh_token: &str) -> Option<RefreshBlock> {
    let key = fingerprint(refresh_token);
    with_failures(|failures| match failures.get(&key) {
        Some(Failure::Retired) => Some(RefreshBlock::Retired),
        Some(Failure::Transient(at)) if at.elapsed() < TRANSIENT_COOLDOWN => {
            Some(RefreshBlock::CoolingDown)
        }
        _ => None,
    })
}

/// Record a failed refresh. Returns `true` the first time this branch is
/// retired, so the caller can tell the user once instead of once per turn.
pub(crate) fn record_failure(refresh_token: &str, error: &ApiError) -> bool {
    let key = fingerprint(refresh_token);
    let terminal = is_terminal_rejection(error);
    with_failures(|failures| {
        let already_retired = matches!(failures.get(&key), Some(Failure::Retired));
        failures.insert(
            key,
            if terminal {
                Failure::Retired
            } else {
                Failure::Transient(Instant::now())
            },
        );
        terminal && !already_retired
    })
}

/// Forget this branch's failure history — it just worked.
pub(crate) fn record_success(refresh_token: &str) {
    let key = fingerprint(refresh_token);
    with_failures(|failures| failures.remove(&key));
}

/// Whether the token endpoint rejected the *grant* rather than failing to
/// answer. OAuth 2.0 says `invalid_grant` for a refresh token that is expired,
/// revoked, or already rotated past — none of which a retry can undo.
fn is_terminal_rejection(error: &ApiError) -> bool {
    match error {
        ApiError::Api { status, body, .. } => {
            (status.as_u16() == 400 || status.as_u16() == 401)
                && body.to_ascii_lowercase().contains("invalid_grant")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RefreshBlock, TRANSIENT_COOLDOWN, is_terminal_rejection, record_failure, record_success,
        refresh_blocked,
    };
    use crate::error::ApiError;

    fn rejection(status: u16, body: &str) -> ApiError {
        ApiError::Api {
            status: reqwest::StatusCode::from_u16(status).expect("status"),
            error_type: None,
            message: None,
            body: body.to_string(),
            retryable: false,
            retry_after: None,
        }
    }

    /// Serialize: the failure map is process-global.
    fn gate_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The whole point of the gate: a branch the endpoint rejected is never
    /// spent again, so a dead token stops costing a 400 per turn.
    #[test]
    fn an_invalid_grant_retires_only_that_branch() {
        let _lock = gate_lock();
        let dead = "dead-branch-token";
        let live = "live-branch-token";
        record_success(dead);
        record_success(live);

        let first = record_failure(dead, &rejection(400, r#"{"error": "invalid_grant"}"#));
        assert!(first, "the first retirement is the one worth reporting");
        assert!(!record_failure(
            dead,
            &rejection(400, r#"{"error": "invalid_grant"}"#)
        ));

        assert_eq!(refresh_blocked(dead), Some(RefreshBlock::Retired));
        assert_eq!(
            refresh_blocked(live),
            None,
            "a fresh sign-in mints a different token and must not inherit the block"
        );

        record_success(dead);
        assert_eq!(refresh_blocked(dead), None, "success clears the history");
    }

    /// A network failure is not a revoked grant: it cools down and is retried,
    /// which is why the two are recorded differently.
    #[test]
    fn a_transient_failure_cools_down_instead_of_retiring() {
        let _lock = gate_lock();
        let token = "transient-branch-token";
        record_success(token);

        assert!(!record_failure(
            token,
            &ApiError::Auth("connection refused".to_string())
        ));
        assert_eq!(refresh_blocked(token), Some(RefreshBlock::CoolingDown));
        assert!(
            TRANSIENT_COOLDOWN.as_secs() > 0,
            "a zero cooldown would hammer the endpoint"
        );
        record_success(token);
    }

    /// Only the grant rejection is terminal. A 400 that is *not* `invalid_grant`
    /// (a malformed request zo can fix, say) must stay retryable, and a 500 is
    /// the server's problem, not the token's.
    #[test]
    fn only_an_invalid_grant_rejection_is_terminal() {
        assert!(is_terminal_rejection(&rejection(
            400,
            r#"{"error": "invalid_grant", "error_description": "Refresh token not found or invalid"}"#
        )));
        assert!(is_terminal_rejection(&rejection(401, "invalid_grant")));
        assert!(!is_terminal_rejection(&rejection(
            400,
            r#"{"error": "invalid_request"}"#
        )));
        assert!(!is_terminal_rejection(&rejection(500, "invalid_grant")));
        assert!(!is_terminal_rejection(&ApiError::Auth("offline".into())));
    }
}
