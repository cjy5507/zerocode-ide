//! Shared-secret authentication for `zo serve`.
//!
//! `zo serve` lifts the in-memory session pool onto a TCP socket so a client
//! can detach and reattach. By default it binds loopback (`127.0.0.1`), where
//! the OS already restricts reach to local processes, so no token is required —
//! the dev ergonomics of `zo serve` + `zo attach` stay zero-config.
//!
//! The moment the bind address is **not** loopback (`0.0.0.0`, a LAN IP, a
//! hostname) the server is reachable by other machines, and an *unauthenticated*
//! agent server is effectively a remote-code-execution box: any peer could drive
//! turns, read the transcript, or run tools. So binding non-loopback **requires**
//! a shared secret in `ZO_SERVE_TOKEN`; the server refuses to start otherwise
//! ([`startup_gate`]). When a token is configured (on any address), every request
//! must present it ([`authorize`]).
//!
//! The token travels in the optional `token` field of each
//! [`RpcRequest`](crate::serve_protocol::RpcRequest) and is checked with a
//! constant-time compare so a wrong guess leaks no timing signal. Both
//! `zo serve` and `zo attach` read the same env var ([`token_from_env`]),
//! so attaching to a guarded server is also zero-config on the same machine.
//!
//! Everything here is pure (env read aside) and unit-tested; the transport
//! modules ([`crate::serve`], [`crate::attach`], [`crate::attach_tui`]) only
//! plumb the verdicts.

use std::net::{IpAddr, SocketAddr};

/// Environment variable carrying the full-access shared secret for
/// `zo serve`/`attach`.
pub(crate) const TOKEN_ENV: &str = "ZO_SERVE_TOKEN";
/// Optional read-only shared secret. A client may put this value in its
/// `token` request field to list/load sessions without being able to drive
/// turns, answer permissions, or mutate session state.
pub(crate) const READ_TOKEN_ENV: &str = "ZO_SERVE_READ_TOKEN";

/// Read the configured shared secret from the environment, normalising an
/// unset-or-blank value to `None` — a whitespace-only token is treated as *no
/// token* rather than as a secret a client might accidentally match. The raw
/// value is returned unchanged (not trimmed) so server and client, reading the
/// same variable, always agree byte-for-byte.
pub(crate) fn token_from_env() -> Option<String> {
    token_from_named_env(TOKEN_ENV)
}

fn token_from_named_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

/// Capability required by an RPC method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServeCapability {
    /// Read session metadata/history/job status only.
    Read,
    /// Full interactive control of the server.
    Full,
}

/// Shared-secret policy for `zo serve`.
///
/// `ZO_SERVE_TOKEN` remains the backwards-compatible full-access token.
/// `ZO_SERVE_READ_TOKEN` is optional and can be handed to clients that only
/// need to inspect sessions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ServeAuthPolicy {
    full_token: Option<String>,
    read_token: Option<String>,
}

impl ServeAuthPolicy {
    #[must_use]
    pub(crate) fn from_env() -> Self {
        Self {
            full_token: token_from_named_env(TOKEN_ENV),
            read_token: token_from_named_env(READ_TOKEN_ENV),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn new(full_token: Option<String>, read_token: Option<String>) -> Self {
        Self {
            full_token,
            read_token,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn open() -> Self {
        Self::default()
    }

    #[must_use]
    pub(crate) fn has_any_token(&self) -> bool {
        self.full_token.is_some() || self.read_token.is_some()
    }

    #[must_use]
    pub(crate) fn has_read_token(&self) -> bool {
        self.read_token.is_some()
    }

    #[must_use]
    pub(crate) fn authorize(
        &self,
        provided: Option<&str>,
        required: ServeCapability,
    ) -> AuthOutcome {
        if !self.has_any_token() {
            return AuthOutcome::Allowed;
        }

        let Some(candidate) = provided else {
            return AuthOutcome::Rejected;
        };

        if self
            .full_token
            .as_deref()
            .is_some_and(|secret| constant_time_eq(secret.as_bytes(), candidate.as_bytes()))
        {
            return AuthOutcome::Allowed;
        }

        if self
            .read_token
            .as_deref()
            .is_some_and(|secret| constant_time_eq(secret.as_bytes(), candidate.as_bytes()))
        {
            return match required {
                ServeCapability::Read => AuthOutcome::Allowed,
                ServeCapability::Full => AuthOutcome::InsufficientCapability,
            };
        }

        AuthOutcome::Rejected
    }
}

/// Decide whether a bind address points only at the loopback interface.
///
/// Gates the token requirement: a loopback bind is local-only and may run
/// tokenless; anything else is network-reachable and must be guarded.
///
/// Recognises `127.0.0.1:8787`, a bare `127.0.0.1`, bracketed IPv6
/// (`[::1]:8787`), and the `localhost` hostname. A wildcard bind (`0.0.0.0`,
/// `[::]`) is **not** loopback — it accepts external peers. An unrecognised
/// hostname is treated as non-loopback: the safe default is to demand a token
/// rather than silently expose the server.
pub(crate) fn host_is_loopback(bind_addr: &str) -> bool {
    if let Ok(sock) = bind_addr.parse::<SocketAddr>() {
        return sock.ip().is_loopback();
    }
    if let Ok(ip) = bind_addr.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    // Hostname form (possibly `host:port`): only `localhost` is known-loopback.
    let host = bind_addr
        .rsplit_once(':')
        .map_or(bind_addr, |(host, _)| host);
    host.eq_ignore_ascii_case("localhost")
}

/// Decide, before binding, whether the server may start on `bind_addr` with the
/// given `token`. A loopback bind always may; a network-reachable bind may only
/// if a token is configured. On refusal the `Err` message tells the operator
/// exactly how to fix it.
#[cfg(test)]
pub(crate) fn startup_gate(bind_addr: &str, token: Option<&str>) -> Result<(), String> {
    startup_gate_for_auth(bind_addr, token.is_some())
}

/// Same startup gate as [`startup_gate`], but accepts the already-computed
/// policy state so scoped tokens also count as authentication.
pub(crate) fn startup_gate_for_auth(
    bind_addr: &str,
    authentication_configured: bool,
) -> Result<(), String> {
    if host_is_loopback(bind_addr) || authentication_configured {
        return Ok(());
    }
    Err(format!(
        "zo serve: refusing to bind {bind_addr} without authentication.\n  \
         This address is reachable from other machines, and an unauthenticated\n  \
         agent server would let any peer run tools and read your sessions.\n  \
         Set a shared secret first, e.g.:\n      \
         {TOKEN_ENV}=$(openssl rand -hex 16) zo serve --bind {bind_addr}\n  \
         (loopback binds such as 127.0.0.1 need no token.)"
    ))
}

/// Verdict from [`authorize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthOutcome {
    /// The request may proceed (no token configured, or the token matched).
    Allowed,
    /// The request is rejected (token configured but missing or wrong).
    Rejected,
    /// The token is valid but lacks the requested capability.
    InsufficientCapability,
}

/// Authorise one request against the server's configured secret.
///
/// - No secret configured (`expected = None`) → [`AuthOutcome::Allowed`]: the
///   server is tokenless (a loopback dev server), so every request passes.
/// - Secret configured but the request carried none → [`AuthOutcome::Rejected`].
/// - Both present → a constant-time comparison decides.
#[cfg(test)]
pub(crate) fn authorize(expected: Option<&str>, provided: Option<&str>) -> AuthOutcome {
    ServeAuthPolicy::new(expected.map(str::to_owned), None)
        .authorize(provided, ServeCapability::Full)
}

/// Compare two byte strings in time independent of *where* they first differ,
/// so a network peer can't binary-search a secret by timing rejections. The
/// length is allowed to leak (standard for shared-secret checks); only the
/// content comparison is constant-time.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_addresses_are_recognised() {
        assert!(host_is_loopback("127.0.0.1:8787"));
        assert!(host_is_loopback("127.0.0.1"));
        assert!(host_is_loopback("[::1]:8787"));
        assert!(host_is_loopback("localhost:8787"));
        assert!(host_is_loopback("localhost"));
        assert!(host_is_loopback("LOCALHOST:9000"));
    }

    #[test]
    fn network_reachable_addresses_are_not_loopback() {
        // Wildcard binds accept external peers — never loopback.
        assert!(!host_is_loopback("0.0.0.0:8787"));
        assert!(!host_is_loopback("[::]:8787"));
        // A concrete LAN address.
        assert!(!host_is_loopback("192.168.1.10:8787"));
        // Unknown hostname → safe default (treated as exposed).
        assert!(!host_is_loopback("my-laptop.local:8787"));
    }

    #[test]
    fn startup_gate_allows_loopback_with_or_without_token() {
        assert!(startup_gate("127.0.0.1:8787", None).is_ok());
        assert!(startup_gate("127.0.0.1:8787", Some("s3cret")).is_ok());
        assert!(startup_gate("[::1]:8787", None).is_ok());
    }

    #[test]
    fn startup_gate_allows_non_loopback_only_with_a_token() {
        assert!(startup_gate("0.0.0.0:8787", Some("s3cret")).is_ok());
        assert!(startup_gate_for_auth("0.0.0.0:8787", true).is_ok());
        let refusal = startup_gate("0.0.0.0:8787", None)
            .expect_err("non-loopback bind without a token must be refused");
        // The message must name the env var so the operator knows the fix.
        assert!(refusal.contains(TOKEN_ENV));
        assert!(refusal.contains("0.0.0.0:8787"));
    }

    #[test]
    fn authorize_open_when_no_secret_configured() {
        assert_eq!(authorize(None, None), AuthOutcome::Allowed);
        assert_eq!(authorize(None, Some("anything")), AuthOutcome::Allowed);
    }

    #[test]
    fn authorize_rejects_missing_or_wrong_token() {
        assert_eq!(authorize(Some("s3cret"), None), AuthOutcome::Rejected);
        assert_eq!(
            authorize(Some("s3cret"), Some("wrong")),
            AuthOutcome::Rejected
        );
        // A correct prefix is still wrong (length differs).
        assert_eq!(
            authorize(Some("s3cret"), Some("s3cre")),
            AuthOutcome::Rejected
        );
        assert_eq!(
            authorize(Some("s3cret"), Some("s3cretX")),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn authorize_allows_exact_match() {
        assert_eq!(
            authorize(Some("s3cret"), Some("s3cret")),
            AuthOutcome::Allowed
        );
    }

    #[test]
    fn scoped_policy_allows_read_token_only_for_read_methods() {
        let policy = ServeAuthPolicy::new(
            Some("full-token".to_string()),
            Some("read-token".to_string()),
        );

        assert_eq!(
            policy.authorize(Some("read-token"), ServeCapability::Read),
            AuthOutcome::Allowed
        );
        assert_eq!(
            policy.authorize(Some("read-token"), ServeCapability::Full),
            AuthOutcome::InsufficientCapability
        );
        assert_eq!(
            policy.authorize(Some("full-token"), ServeCapability::Read),
            AuthOutcome::Allowed
        );
        assert_eq!(
            policy.authorize(Some("full-token"), ServeCapability::Full),
            AuthOutcome::Allowed
        );
    }

    #[test]
    fn scoped_policy_rejects_missing_or_unknown_tokens() {
        let policy = ServeAuthPolicy::new(
            Some("full-token".to_string()),
            Some("read-token".to_string()),
        );

        assert_eq!(
            policy.authorize(None, ServeCapability::Read),
            AuthOutcome::Rejected
        );
        assert_eq!(
            policy.authorize(Some("wrong"), ServeCapability::Read),
            AuthOutcome::Rejected
        );
    }

    #[test]
    fn constant_time_eq_matches_only_identical_bytes() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"ab", b"abc"));
    }
}
