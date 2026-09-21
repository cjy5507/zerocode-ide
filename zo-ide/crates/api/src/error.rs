use std::env::VarError;
use std::fmt::{Display, Formatter};
use std::time::Duration;

// Every human-facing recovery hint below is emitted after this marker, and the
// shared classifier stops reading there. Same symbol on both sides so the
// producer and the reader cannot drift — see its docs for what drifting cost.
use core_types::retry_signal::{
    HINT_SEPARATOR, RESET_HINT_TOKEN, UNSUPPORTED_VISION_HINT_BODY, human_reset_wait,
    is_unsupported_vision_text, is_usage_limit_text,
};

#[derive(Debug)]
pub enum ApiError {
    MissingCredentials {
        provider: &'static str,
        env_vars: &'static [&'static str],
    },
    UnsupportedProvider {
        provider: &'static str,
        gate_env: &'static str,
    },
    MissingAuthRouteCredentials {
        provider: &'static str,
        route: &'static str,
    },
    UnsupportedAuthRoute {
        provider: &'static str,
        route: &'static str,
    },
    ExpiredOAuthToken,
    Auth(String),
    InvalidApiKeyEnv(VarError),
    Http(reqwest::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    Api {
        status: reqwest::StatusCode,
        error_type: Option<String>,
        message: Option<String>,
        body: String,
        retryable: bool,
        retry_after: Option<Duration>,
    },
    StreamApi {
        error_type: Option<String>,
        message: Option<String>,
        body: String,
        retryable: bool,
    },
    RetriesExhausted {
        attempts: u32,
        last_error: Box<ApiError>,
    },
    InvalidSseFrame(&'static str),
    BackoffOverflow {
        attempt: u32,
        base_delay: Duration,
    },
}

/// Whose capacity ran out behind a [`ProviderErrorClass::RateLimit`].
///
/// The two answers demand opposite recoveries, so every layer that reacts to a
/// capacity stall has to be able to tell them apart. See
/// [`core_types::retry_signal::RetrySignal`] for the same distinction on the
/// flattened-display path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityScope {
    /// **This account's** window is exhausted — HTTP 429 / `rate_limit_error`.
    /// Only time lifts it, and every model on this provider shares the window,
    /// so a swap inside the provider just moves the 429.
    Account,
    /// **The provider** shed the request for its own capacity — HTTP 529 /
    /// `overloaded_error`. Independent of the account window (it fires at 2 %
    /// utilization), usually clears in seconds, and a lighter model or another
    /// provider is the recovery that actually works.
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorClass {
    /// A capacity refusal. `scope` decides the recovery: ride the window out
    /// ([`CapacityScope::Account`]) or escape to another model
    /// ([`CapacityScope::Provider`]). Existing `RateLimit { .. }` matches keep
    /// working — they only ask "was this capacity?", which both scopes answer.
    RateLimit {
        retry_after: Option<Duration>,
        scope: CapacityScope,
    },
    Transient,
    AuthExpired,
    ContextOverflow,
    InvalidToolProtocol,
    InvalidToolSchema,
    SafetyBlocked,
    /// A gateway's content filter refused the request for its text
    /// (`core_types::retry_signal::is_content_refusal_text`). The same bytes
    /// are refused again, so it is never retried as is; the runtime may drop
    /// what it attached on its own and ask once more.
    ContentRefused,
    NonRetryable,
}

impl ProviderErrorClass {
    /// Convenience constructor for an account-window refusal (429).
    #[must_use]
    pub const fn account_rate_limit(retry_after: Option<Duration>) -> Self {
        Self::RateLimit {
            retry_after,
            scope: CapacityScope::Account,
        }
    }

    /// Convenience constructor for a provider-capacity refusal (529).
    #[must_use]
    pub const fn provider_overloaded(retry_after: Option<Duration>) -> Self {
        Self::RateLimit {
            retry_after,
            scope: CapacityScope::Provider,
        }
    }

    /// Whose capacity ran out, or `None` when this is not a capacity refusal.
    #[must_use]
    pub const fn capacity_scope(self) -> Option<CapacityScope> {
        match self {
            Self::RateLimit { scope, .. } => Some(scope),
            Self::Transient
            | Self::AuthExpired
            | Self::ContextOverflow
            | Self::InvalidToolProtocol
            | Self::InvalidToolSchema
            | Self::SafetyBlocked
            | Self::ContentRefused
            | Self::NonRetryable => None,
        }
    }

    /// True when the refusal is the *provider* shedding load rather than this
    /// account's window being spent.
    #[must_use]
    pub const fn is_provider_overload(self) -> bool {
        matches!(
            self,
            Self::RateLimit {
                scope: CapacityScope::Provider,
                ..
            }
        )
    }
}

/// Whether a `reqwest` failure is a transport hiccup rather than a verdict the
/// provider actually delivered.
///
/// `is_body`/`is_decode` are the ones that were missing, and the ones that bite:
/// they fire when the response opened cleanly and then failed part-way through
/// the body — a reset mid-SSE-stream, a chunked body that ends short. That
/// surfaces as `error decoding response body`, and treating it as permanent
/// killed sub-agents that had already done dozens of tool calls. A failure that
/// happened in transit is no more final than a connect or timeout failure, so
/// it retries alongside them.
///
/// One predicate, consulted by both the class mapping and the retry test, so the
/// two can never drift into disagreeing about what is worth retrying.
fn is_transient_transport_error(error: &reqwest::Error) -> bool {
    error.is_connect()
        || error.is_timeout()
        || error.is_request()
        || error.is_body()
        || error.is_decode()
}

impl ApiError {
    #[must_use]
    pub const fn missing_credentials(
        provider: &'static str,
        env_vars: &'static [&'static str],
    ) -> Self {
        Self::MissingCredentials { provider, env_vars }
    }

    #[must_use]
    pub const fn unsupported_provider(provider: &'static str, gate_env: &'static str) -> Self {
        Self::UnsupportedProvider { provider, gate_env }
    }

    #[must_use]
    pub const fn missing_auth_route_credentials(
        provider: &'static str,
        route: &'static str,
    ) -> Self {
        Self::MissingAuthRouteCredentials { provider, route }
    }

    #[must_use]
    pub const fn unsupported_auth_route(
        provider: &'static str,
        route: &'static str,
    ) -> Self {
        Self::UnsupportedAuthRoute { provider, route }
    }

    /// A streaming response went silent for longer than the idle budget.
    ///
    /// Marked retryable so the caller's retry policy can re-establish the
    /// stream instead of hanging forever on a quietly-reasoning backend.
    #[must_use]
    pub fn stream_idle_timeout(idle: Duration) -> Self {
        Self::StreamApi {
            error_type: Some("stream_idle_timeout".to_string()),
            message: Some(format!(
                "no stream data for {}s; backend went silent",
                idle.as_secs()
            )),
            body: String::new(),
            retryable: true,
        }
    }

    /// A provider kept the transport alive but produced no task action before
    /// the startup deadline. This is distinct from a byte-level idle timeout:
    /// keep-alive frames may have arrived, but they are not model progress.
    #[must_use]
    pub fn stream_startup_no_progress(budget: Duration, reasoning_extended: bool) -> Self {
        let extension = if reasoning_extended {
            " after one reasoning-based extension"
        } else {
            ""
        };
        Self::StreamApi {
            error_type: Some("stream_startup_no_progress".to_string()),
            message: Some(format!(
                "no text or tool action within {}s{extension}; transport keep-alives are not progress",
                budget.as_secs()
            )),
            body: String::new(),
            retryable: true,
        }
    }

    /// A pre-commit stream restart could not re-establish the connection
    /// within the remaining restart wall-clock budget (the reopen request was
    /// accepted but never answered). Retryable for parity with
    /// [`Self::stream_idle_timeout`], whose budget-exhausted path surfaces the
    /// same way; the caller's own retry policy decides what happens next.
    #[must_use]
    pub fn stream_restart_timeout(remaining: Duration) -> Self {
        Self::StreamApi {
            error_type: Some("stream_restart_timeout".to_string()),
            message: Some(format!(
                "stream reopen did not complete within the remaining {}ms restart budget",
                remaining.as_millis()
            )),
            body: String::new(),
            retryable: true,
        }
    }

    /// A 429 raised **before any request left this process**, shaped exactly
    /// like the one the account window answers with on the wire.
    ///
    /// A caller that already knows the window is parked — the provider
    /// client's pre-send door, reading [`crate::quota`] — holds a cool-down,
    /// not a response. What it must hand up is nevertheless indistinguishable
    /// from a real 429, because every reader downstream keys off the fields a
    /// wire 429 carries: [`Self::provider_error_class`] reads the status for
    /// the quota escape's scope, [`Self::is_rate_limit`] reads it again, and
    /// [`Display`] turns `retry_after` into the [`RESET_HINT_TOKEN`] the
    /// runtime's retry classifier parses back out of the flattened text.
    /// Minting the shape here, beside the wire path that builds the same
    /// variant, is what keeps the two the same error; assembled at the call
    /// site, one missing field makes the wall unreadable to the very escape it
    /// exists to trigger.
    #[must_use]
    pub fn rate_limited_before_send(provider: &str, parked_for: Duration) -> Self {
        Self::Api {
            status: reqwest::StatusCode::TOO_MANY_REQUESTS,
            error_type: Some("rate_limit_error".to_string()),
            message: Some(format!(
                "{provider} is parked by a rate limit this machine already took, \
                 so no request was sent; the window clears in {}",
                human_reset_wait(parked_for)
            )),
            body: String::new(),
            retryable: true,
            retry_after: Some(parked_for),
        }
    }

    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Api { retry_after, .. } => *retry_after,
            Self::RetriesExhausted { last_error, .. } => last_error.retry_after(),
            _ => None,
        }
    }

    /// The prompt-token ceiling this error named, when it named one.
    ///
    /// See [`context_overflow_ceiling_tokens`]: the provider's own number is the
    /// only authority on what the wire allows, and a client window guessed above
    /// it disables every compaction threshold derived from it.
    #[must_use]
    pub fn context_overflow_ceiling(&self) -> Option<u64> {
        match self {
            Self::Api { message, body, .. } => message
                .as_deref()
                .and_then(context_overflow_ceiling_tokens)
                .or_else(|| context_overflow_ceiling_tokens(body)),
            Self::StreamApi { message, body, .. } => message
                .as_deref()
                .and_then(context_overflow_ceiling_tokens)
                .or_else(|| context_overflow_ceiling_tokens(body)),
            Self::RetriesExhausted { last_error, .. } => last_error.context_overflow_ceiling(),
            _ => None,
        }
    }

    /// True when this error indicates the model does not support image (vision) inputs.
    #[must_use]
    pub fn is_unsupported_vision(&self) -> bool {
        let text = match self {
            Self::Api {
                error_type,
                message,
                body,
                ..
            }
            | Self::StreamApi {
                error_type,
                message,
                body,
                ..
            } => [
                error_type.as_deref(),
                message.as_deref(),
                Some(body.as_str()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
            Self::RetriesExhausted { last_error, .. } => return last_error.is_unsupported_vision(),
            _ => return false,
        };
        is_unsupported_vision_text(&text.to_ascii_lowercase())
    }

    #[must_use]
    pub fn provider_error_class(&self) -> ProviderErrorClass {
        match self {
            Self::Api {
                status,
                error_type,
                message,
                body,
                retry_after,
                ..
            } => {
                let parts = [
                    error_type.as_deref(),
                    message.as_deref(),
                    Some(body.as_str()),
                ];
                if status.as_u16() == 401 {
                    // A 401 that rejects the *client* (provider fingerprint /
                    // WAF whitelist), not the credential, can never be fixed by
                    // an OAuth refresh or `zo login` — fail fast instead of
                    // routing it through the auth-recovery retry.
                    if is_client_rejection_text(parts.iter().copied().flatten()) {
                        return ProviderErrorClass::NonRetryable;
                    }
                    return ProviderErrorClass::AuthExpired;
                }
                // Status first, wording second: 429 and 529 are unambiguous about
                // whose capacity ran out, and only a status-less relay error has
                // to be read from prose.
                if let Some(scope) = capacity_scope_of(Some(status.as_u16()), parts.iter().copied())
                {
                    return ProviderErrorClass::RateLimit {
                        retry_after: *retry_after,
                        scope,
                    };
                }
                // 413 by STATUS, ahead of the text classifier: the body is the
                // one thing this stack can still act on (compact, then
                // re-send), and a gateway is free to word its payload-limit
                // rejection however it likes. Matching only on wording left an
                // Anthropic `request_too_large` ("Request exceeds the maximum
                // size" — none of the token-overflow phrases) falling through
                // to `NonRetryable`, which killed the turn outright with a raw
                // transport error and no recovery attempt.
                if status.as_u16() == 413 {
                    return ProviderErrorClass::ContextOverflow;
                }
                if let Some(class) = classify_provider_error_text(parts.into_iter().flatten()) {
                    return class;
                }
                let code = status.as_u16();
                if matches!(code, 408 | 409) || code >= 500 {
                    ProviderErrorClass::Transient
                } else {
                    ProviderErrorClass::NonRetryable
                }
            }
            Self::StreamApi {
                error_type,
                message,
                body,
                retryable,
            } => {
                let parts = [
                    error_type.as_deref(),
                    message.as_deref(),
                    Some(body.as_str()),
                ];
                // An SSE `error` frame rides an HTTP 200, so there is no status to
                // read: the frame's own `type` (`overloaded_error` vs
                // `rate_limit_error`) is the scope evidence. This is the seam the
                // "hi 쳤는데 Overloaded" turn came through.
                if let Some(scope) = capacity_scope_of(None, parts.iter().copied()) {
                    return ProviderErrorClass::RateLimit {
                        retry_after: None,
                        scope,
                    };
                }
                if let Some(class) = classify_provider_error_text(parts.into_iter().flatten()) {
                    return class;
                }
                if *retryable {
                    ProviderErrorClass::Transient
                } else {
                    ProviderErrorClass::NonRetryable
                }
            }
            Self::Http(error) if is_transient_transport_error(error) => {
                ProviderErrorClass::Transient
            }
            Self::RetriesExhausted { last_error, .. } => last_error.provider_error_class(),
            Self::MissingCredentials { .. }
            | Self::UnsupportedProvider { .. }
            | Self::MissingAuthRouteCredentials { .. }
            | Self::UnsupportedAuthRoute { .. }
            | Self::ExpiredOAuthToken
            | Self::Auth(_)
            | Self::InvalidApiKeyEnv(_) => ProviderErrorClass::AuthExpired,
            Self::Http(_)
            | Self::Io(_)
            | Self::Json(_)
            | Self::InvalidSseFrame(_)
            | Self::BackoffOverflow { .. } => ProviderErrorClass::NonRetryable,
        }
    }

    /// True when the failure is a provider rate-limit / overload signal
    /// (HTTP 429 or 529), at any retry-exhaustion depth. Classified from the
    /// structured error — the status code for the non-stream path and the
    /// error type/message for the streaming path — instead of substring-matching
    /// a flattened display string, so the adaptive governor can tell a genuine
    /// throttle apart from an auth/validation error that must fail fast.
    #[must_use]
    pub fn is_rate_limit(&self) -> bool {
        match self {
            Self::Api { status, .. } => {
                let code = status.as_u16();
                code == 429 || code == 529
            }
            Self::StreamApi {
                error_type,
                message,
                body,
                ..
            } => {
                let haystack = error_type
                    .as_deref()
                    .into_iter()
                    .chain(message.as_deref())
                    .chain(std::iter::once(body.as_str()));
                // The capacity vocabulary (429 / 529 / overloaded / rate limit /
                // too many requests) is shared with the runtime retry + stream
                // layers via `core_types::retry_signal`, so a new overload
                // wording is recognised everywhere at once. The structured `Api`
                // arm above still classifies from the HTTP status code directly.
                haystack.into_iter().any(|part| {
                    core_types::retry_signal::is_capacity_text(&part.to_ascii_lowercase())
                })
            }
            Self::RetriesExhausted { last_error, .. } => last_error.is_rate_limit(),
            _ => false,
        }
    }

    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http(error) => is_transient_transport_error(error),
            Self::Api { retryable, .. } | Self::StreamApi { retryable, .. } => {
                *retryable
                    && !matches!(
                        self.provider_error_class(),
                        ProviderErrorClass::ContextOverflow | ProviderErrorClass::ContentRefused
                    )
            }
            Self::RetriesExhausted { last_error, .. } => last_error.is_retryable(),
            Self::MissingCredentials { .. }
            | Self::UnsupportedProvider { .. }
            | Self::MissingAuthRouteCredentials { .. }
            | Self::UnsupportedAuthRoute { .. }
            | Self::ExpiredOAuthToken
            | Self::Auth(_)
            | Self::InvalidApiKeyEnv(_)
            | Self::Io(_)
            | Self::Json(_)
            | Self::InvalidSseFrame(_)
            | Self::BackoffOverflow { .. } => false,
        }
    }

    /// True when the failure is an HTTP 401 (expired/invalid credentials), at
    /// any retry-exhaustion depth. Drives a one-shot OAuth refresh + retry so a
    /// bearer that lapses mid-turn doesn't kill the turn until a restart.
    #[must_use]
    pub fn is_unauthorized(&self) -> bool {
        match self {
            Self::Api {
                status,
                error_type,
                message,
                body,
                ..
            } => {
                // A server 401 normally means a stale bearer worth one refresh +
                // retry. But a *client-rejection* 401 (provider fingerprint /
                // whitelist) is not a credential problem — refreshing and
                // retrying with the same client identity just 401s again, so it
                // must not drive the OAuth-recovery path.
                status.as_u16() == 401
                    && !is_client_rejection_text(
                        [error_type.as_deref(), message.as_deref(), Some(body.as_str())]
                            .into_iter()
                            .flatten(),
                    )
            }
            Self::RetriesExhausted { last_error, .. } => last_error.is_unauthorized(),
            _ => false,
        }
    }
}

/// Classify a refusal into a [`CapacityScope`], or `None` when it is not a
/// capacity refusal at all.
///
/// `status` is the HTTP status when one exists (the non-stream path); an SSE
/// `error` frame has none, so it passes `None` and is judged purely on the
/// frame's `type`/`message`/`body` wording. The account arm is checked first for
/// the same reason [`core_types::retry_signal::classify_error_text`] does it:
/// reading a real 429 as an overload would swap models on an exhausted account,
/// where the swap target shares the window and 429s too.
fn capacity_scope_of<'a>(
    status: Option<u16>,
    parts: impl IntoIterator<Item = Option<&'a str>>,
) -> Option<CapacityScope> {
    if status == Some(429) {
        return Some(CapacityScope::Account);
    }
    if status == Some(529) {
        return Some(CapacityScope::Provider);
    }
    let mut overloaded = false;
    for part in parts.into_iter().flatten() {
        let lower = part.to_ascii_lowercase();
        if core_types::retry_signal::is_account_rate_limit_text(&lower) {
            return Some(CapacityScope::Account);
        }
        overloaded = overloaded || core_types::retry_signal::is_overloaded_text(&lower);
    }
    overloaded.then_some(CapacityScope::Provider)
}

/// True when a 401 body signals that the *client itself* was rejected — the
/// provider's fingerprint check / WAF whitelist refused the request before the
/// credential was ever weighed. Relay backends that only accept traffic
/// matching an official client's wire image (e.g. agentrouter, which mimics
/// the Claude Code client) return this with a distinctive
/// `unauthorized_client` type. Such a failure is not fixable by re-login: the
/// token is fine, the caller's identity is not.
fn is_client_rejection_text<'a>(parts: impl IntoIterator<Item = &'a str>) -> bool {
    parts.into_iter().any(|part| {
        let part = part.to_ascii_lowercase();
        part.contains("unauthorized_client")
            || part.contains("unauthorized client")
            || part.contains("client_not_allowed")
            || part.contains("client not allowed")
    })
}

/// The ceiling a provider named while rejecting an oversized prompt, in tokens.
///
/// Anthropic states it outright — `prompt is too long: 211352 tokens > 200000
/// maximum` — and that number is the only trustworthy statement of what the wire
/// actually allows. A client-side window comes from a catalog or a name-shape
/// guess and cannot know that an account, tier, or route serves less; when it
/// guesses high, *every* compaction threshold derived from it sits above the real
/// ceiling and nothing fires until the request 400s. Learning the number from the
/// rejection turns a recurring wall into a one-time correction.
///
/// `None` when the message names no ceiling — the caller keeps its own window.
#[must_use]
pub fn context_overflow_ceiling_tokens(text: &str) -> Option<u64> {
    // `… > 200000 maximum` — anchored on the comparison so a token count from
    // elsewhere in the message (the offending size, itself larger) cannot be
    // mistaken for the limit.
    let (_, after) = text.split_once('>')?;
    let digits: String = after
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    let ceiling = digits.parse::<u64>().ok()?;
    let rest = after.trim_start()[digits.len()..].trim_start();
    // Require the limit wording so an unrelated `>` in a message body cannot
    // silently shrink the session's window.
    rest.starts_with("maximum")
        .then_some(ceiling)
        .filter(|&ceiling| ceiling > 0)
}

fn classify_provider_error_text<'a>(
    parts: impl IntoIterator<Item = &'a str>,
) -> Option<ProviderErrorClass> {
    let mut text = String::new();
    for part in parts {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&part.to_ascii_lowercase());
    }

    if core_types::retry_signal::is_content_refusal_text(&text) {
        return Some(ProviderErrorClass::ContentRefused);
    }
    if text.contains("safety_blocked")
        || (text.contains("safety") && (text.contains("block") || text.contains("finish_reason")))
    {
        return Some(ProviderErrorClass::SafetyBlocked);
    }
    if core_types::retry_signal::is_request_too_large_text(&text)
        || text.contains("context_length_exceeded")
        || text.contains("context window")
        || text.contains("context length")
        || text.contains("maximum context")
        || text.contains("too many tokens")
        || text.contains("token limit")
        || text.contains("exceeds the context")
        // Anthropic's own wording, verbatim: `prompt is too long: 211352 tokens
        // > 200000 maximum`. It matches none of the phrases above, so the single
        // most common overflow error in the product classified as `NonRetryable`
        // — the compact-and-resend recovery existed and never once fired for it,
        // and the turn simply died. Sub-agents hit it constantly: they grind
        // through tool output for up to 64 iterations with nobody watching, and
        // the client-side budget cannot see a wire ceiling below its own.
        || text.contains("prompt is too long")
        || text.contains("input is too long")
        || text.contains("messages too long")
    {
        return Some(ProviderErrorClass::ContextOverflow);
    }
    if text.contains("thought_signature")
        || text.contains("thoughtsignature")
        || ((text.contains("functioncall")
            || text.contains("function call")
            || text.contains("functionresponse")
            || text.contains("function response"))
            && (text.contains("missing")
                || text.contains("mismatch")
                || text.contains("protocol")
                || text.contains("number of function response parts")))
    {
        return Some(ProviderErrorClass::InvalidToolProtocol);
    }
    if text.contains("function_declarations")
        || text.contains("function declaration")
        || text.contains("tool schema")
        || text.contains("parameters.properties")
        || (text.contains("tools[") && text.contains("schema"))
    {
        return Some(ProviderErrorClass::InvalidToolSchema);
    }
    None
}

/// Body of the explanation appended to every provider-capacity refusal (HTTP 529
/// and the `overloaded_error` SSE frame), shared by both [`Display`] arms so the
/// two surfaces describing one wall can never word it differently.
///
/// The wording names the *scope* on purpose: the reflex on seeing this line was
/// "my rate limit", and the measurement said otherwise (2 % of the account
/// window used while every Opus request was shed). Reaching this text also means
/// the automatic recovery is spent — the stream re-opened itself pre-commit and,
/// if a fallback model is configured, the turn already tried escaping to it.
///
/// Emitted **after** [`HINT_SEPARATOR`], never inline: this prose contains the
/// phrase "rate limit", and a classifier reading it re-derived "account
/// throttle" from the very sentence denying it.
const PROVIDER_CAPACITY_HINT_BODY: &str = "Provider capacity issue — the PROVIDER shed this request. This is NOT your account's rate limit (it fires with the window nearly empty) and NOT a local permission or workspace-trust failure.
  zo already re-opened the stream and, when a fallback model is configured, tried escaping to it. The fastest way through is a lighter model (/model sonnet) or a lower /effort; capacity usually frees up within seconds to minutes.";

/// Explanation appended to a usage-limit refusal (ChatGPT `usage_limit_reached`
/// and its kin): what the wall is, when it lifts in clock words, and the two
/// ways through. Emitted **after** [`HINT_SEPARATOR`] — the seconds the retry
/// layers read stay in the head as [`RESET_HINT_TOKEN`].
fn usage_limit_hint(retry_after: Option<Duration>) -> String {
    let lifts = retry_after.map_or_else(
        || "when its window rolls over".to_string(),
        |wait| format!("in {}", human_reset_wait(wait)),
    );
    format!(
        "This account's usage limit for the provider is reached — a plan window, not a fault, \
         so zo does not retry it; it resets {lifts}. With a quota fallback configured (/smart) \
         the turn continues on another model; otherwise wait for the reset or switch with /model."
    )
}

impl Display for ApiError {
    #[allow(
        clippy::too_many_lines,
        reason = "one cohesive per-variant Display match; splitting it would scatter \
                  the error-message arms (surfaced by a rustc/clippy toolchain bump, \
                  not by a feature change — matches the codebase's existing convention)"
    )]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCredentials { provider, env_vars } => {
                let credential_hint = if env_vars.is_empty() {
                    "set the provider's configured api-key env var".to_string()
                } else {
                    format!("export {}", env_vars.join(" or "))
                };
                write!(
                    f,
                    "missing {provider} credentials; {credential_hint} before calling the {provider} API"
                )?;
                if matches!(*provider, "Anthropic" | "OpenAI" | "Google") {
                    write!(
                        f,
                        "\n\n  No credentials found — authenticate this provider, then retry:\n    • TUI:    /login [provider]      (e.g. /login google, /login openai; bare /login = Claude)\n    • shell:  zo login [provider]"
                    )
                } else {
                    write!(
                        f,
                        "\n\n  No API key found for this OpenAI-compatible adapter. Set the configured auth_env in the shell that launches zo, then retry."
                    )
                }
            }
            Self::UnsupportedProvider { provider, gate_env } => write!(
                f,
                "{provider} adapter is present but disabled in Claude-first mode; configure {provider} (set its API key or base URL) or set {gate_env}=1 to enable the experimental provider path"
            ),
            Self::MissingAuthRouteCredentials { provider, route } => write!(
                f,
                "missing {provider} {route} credentials; authenticate that exact route and retry (automatic credential fallback is disabled for this model)"
            ),
            Self::UnsupportedAuthRoute { provider, route } => write!(
                f,
                "{provider} does not support the explicit {route} authentication route"
            ),
            Self::ExpiredOAuthToken => {
                write!(
                    f,
                    "saved OAuth token is expired and no refresh token is available"
                )
            }
            Self::Auth(message) => write!(f, "auth error: {message}"),
            Self::InvalidApiKeyEnv(error) => {
                write!(f, "failed to read credential environment variable: {error}")
            }
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Api {
                status,
                error_type,
                message,
                body,
                retry_after,
                ..
            } => {
                match (error_type, message) {
                    (Some(error_type), Some(message)) => {
                        write!(f, "api returned {status} ({error_type}): {message}")?;
                    }
                    _ => write!(f, "api returned {status}: {body}")?,
                }
                if let Some(delay) = retry_after {
                    write!(f, "; {}{}", RESET_HINT_TOKEN, delay.as_secs())?;
                }
                if is_usage_limit_text(
                    &[error_type.as_deref(), message.as_deref()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .to_ascii_lowercase(),
                ) {
                    write!(f, "{HINT_SEPARATOR}{}", usage_limit_hint(*retry_after))?;
                }
                // A 401 means the credentials themselves are bad/expired, not a
                // transient fault — point the user at recovery instead of
                // leaving a bare "Invalid authentication credentials".
                if status.as_u16() == 401 {
                    if is_client_rejection_text(
                        [error_type.as_deref(), message.as_deref(), Some(body.as_str())]
                            .into_iter()
                            .flatten(),
                    ) {
                        // The credential is fine; the provider refused *zo* as
                        // an unauthorized client (fingerprint / whitelist). Re-
                        // login cannot fix this — say so plainly instead of
                        // sending the user in a login loop.
                        write!(
                            f,
                            "\n\n  This provider rejected zo as an unauthorized client — not an expired credential. Re-running /login will not help.\n  The endpoint only accepts requests whose wire image matches a whitelisted client (User-Agent / SDK headers).\n  Fix: give this provider a client fingerprint in settings.json — add \"client_fingerprint\": \"codex\" (or \"claude-code\") to its providers[] entry, or set a raw \"user_agent\". If the endpoint accepts generic API clients instead, no fingerprint is needed."
                        )?;
                    } else {
                        // Provider-neutral: a 401 can come from any backend (Claude,
                        // Gemini, ChatGPT, …), so point at re-login generically rather
                        // than assuming an Anthropic credential.
                        write!(
                            f,
                            "\n\n  Authentication failed — credentials expired or invalid.\n  Re-authenticate this model's provider, then retry:\n    • TUI:    /login [provider]      (e.g. /login google, /login openai; bare /login = Claude)\n    • shell:  zo login [provider]"
                        )?;
                    }
                }
                // A 529 is the provider shedding load. The same explanation the
                // streaming arm gives applies verbatim, so both read from one
                // const — the two arms describing the same wall differently is
                // how a user learns to distrust the message.
                if status.as_u16() == 529 {
                    write!(f, "{HINT_SEPARATOR}{PROVIDER_CAPACITY_HINT_BODY}")?;
                }
                // A 413 rejects the request's BYTE size, which is a different
                // ceiling from the model's context window — a turn can sit well
                // inside its token budget and still be refused. Reaching this
                // message means automatic compaction already ran and could not
                // shrink the body enough (or had nothing safe to rewrite), so
                // name the real lever instead of leaving a bare transport error.
                if status.as_u16() == 413 {
                    write!(
                        f,
                        "\n\n  The request BODY exceeded the provider's size limit — a byte ceiling, not the model's context window, so a turn well inside its token budget can still be refused.\n  Automatic compaction already ran and could not shrink it enough. Large attachments are the usual cause: images and pasted binaries cost bytes but few tokens.\n  Fix: /compact to force a rewrite, /clear to start a fresh turn, or re-send without the large attachments."
                    )?;
                }
                if self.is_unsupported_vision() {
                    write!(f, "{HINT_SEPARATOR}{UNSUPPORTED_VISION_HINT_BODY}")?;
                }
                Ok(())
            }
            Self::StreamApi {
                error_type,
                message,
                body,
                ..
            } => {
                match (error_type, message) {
                    (Some(error_type), Some(message)) => {
                        write!(f, "api stream error ({error_type}): {message}")?;
                        if error_type == "overloaded_error" {
                            write!(f, "{HINT_SEPARATOR}{PROVIDER_CAPACITY_HINT_BODY}")?;
                        }
                    }
                    (None, Some(message)) => {
                        write!(f, "api stream error: {message}")?;
                    }
                    _ => write!(f, "api stream error: {body}")?,
                }
                if self.is_unsupported_vision() {
                    write!(f, "{HINT_SEPARATOR}{UNSUPPORTED_VISION_HINT_BODY}")?;
                }
                Ok(())
            }
            Self::RetriesExhausted {
                attempts,
                last_error,
            } => {
                write!(f, "api failed after {attempts} attempts: {last_error}")?;
                let msg = last_error.to_string();
                if msg.contains("429") || msg.contains("rate_limit") || msg.contains("529") {
                    write!(
                        f,
                        "\n\n  Rate limited. Try:\n    1. zo login     (get your own OAuth token)\n    2. --model sonnet  (lower rate limits)\n    3. Wait a minute and retry"
                    )?;
                }
                Ok(())
            }
            Self::InvalidSseFrame(message) => write!(f, "invalid sse frame: {message}"),
            Self::BackoffOverflow {
                attempt,
                base_delay,
            } => write!(
                f,
                "retry backoff overflowed on attempt {attempt} with base delay {base_delay:?}"
            ),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<VarError> for ApiError {
    fn from(value: VarError) -> Self {
        Self::InvalidApiKeyEnv(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn api_401_display_appends_reauth_hint() {
        let err = ApiError::Api {
            status: StatusCode::UNAUTHORIZED,
            error_type: Some("authentication_error".to_string()),
            message: Some("Invalid authentication credentials".to_string()),
            body: String::new(),
            retryable: false,
            retry_after: None,
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("zo login"),
            "401 must point at re-auth: {rendered}"
        );
        assert!(rendered.contains("authentication_error"));
    }

    #[test]
    fn missing_credentials_display_points_at_login() {
        let err = ApiError::missing_credentials("Anthropic", &["ANTHROPIC_API_KEY"]);
        let rendered = err.to_string();
        assert!(
            rendered.contains("ANTHROPIC_API_KEY"),
            "must still name the env var: {rendered}"
        );
        assert!(
            rendered.contains("/login") && rendered.contains("zo login"),
            "missing credentials must point at the login flow: {rendered}"
        );
    }

    #[test]
    fn api_500_display_has_no_reauth_hint() {
        let err = ApiError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error_type: Some("api_error".to_string()),
            message: Some("boom".to_string()),
            body: String::new(),
            retryable: true,
            retry_after: None,
        };
        let rendered = err.to_string();
        assert!(
            !rendered.contains("zo login"),
            "non-401 must not nag about login: {rendered}"
        );
    }

    /// Builds a genuine `reqwest::Error` from a response that declares more
    /// body than it sends. `reqwest::Error` has no public constructor, and the
    /// distinction being tested — a response that opened cleanly and then died
    /// mid-body — only exists once a real transport is involved.
    async fn body_read_failure() -> reqwest::Error {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut scratch = [0u8; 1024];
            let _ = socket.read(&mut scratch).await;
            let body = "half";
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
                        body.len() + 32
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
        });

        let response = reqwest::Client::new()
            .get(format!("http://{addr}"))
            .send()
            .await
            .expect("the response head arrives intact");
        let error = response
            .text()
            .await
            .expect_err("a body shorter than its content-length must fail while being read");
        server.await.unwrap();
        error
    }

    /// A response that dies part-way through its body is a transport hiccup, not
    /// a verdict from the provider: nothing about it says the same request would
    /// fail again. Classifying it as permanent killed sub-agents mid-run with
    /// `error decoding response body` after they had already done real work.
    #[tokio::test]
    async fn a_body_that_fails_mid_read_is_transient_and_retryable() {
        let error = body_read_failure().await;
        assert!(
            error.is_body() || error.is_decode(),
            "the fixture must produce a body-read failure, got {error:?}"
        );

        let api_error = ApiError::Http(error);
        assert!(
            api_error.is_retryable(),
            "a failure that happened in transit must be retried: {api_error}"
        );
        assert_eq!(
            api_error.provider_error_class(),
            ProviderErrorClass::Transient,
            "and it must classify alongside connect/timeout failures"
        );
    }

    /// The two entry points must never disagree about what is worth retrying —
    /// they read the same predicate precisely so they cannot drift apart.
    #[tokio::test]
    async fn transport_retry_classification_agrees_across_both_entry_points() {
        let api_error = ApiError::Http(body_read_failure().await);
        assert_eq!(
            api_error.is_retryable(),
            matches!(
                api_error.provider_error_class(),
                ProviderErrorClass::Transient
            )
        );
    }

    #[test]
    fn stream_idle_timeout_is_retryable() {
        let err = ApiError::stream_idle_timeout(Duration::from_secs(90));
        assert!(err.is_retryable(), "idle timeout must be retryable");
        assert!(err.to_string().contains("90s"));
    }

    #[test]
    fn stream_startup_no_progress_is_retryable_and_distinct_from_transport_idle() {
        let err = ApiError::stream_startup_no_progress(Duration::from_secs(480), true);
        assert!(err.is_retryable());
        let rendered = err.to_string();
        assert!(rendered.contains("stream_startup_no_progress"));
        assert!(rendered.contains("480s"));
        assert!(rendered.contains("reasoning-based extension"));
    }

    #[test]
    fn is_unauthorized_detects_server_401_only() {
        let api = |status| ApiError::Api {
            status,
            error_type: None,
            message: None,
            body: String::new(),
            retryable: false,
            retry_after: None,
        };
        assert!(api(reqwest::StatusCode::UNAUTHORIZED).is_unauthorized());
        assert!(!api(reqwest::StatusCode::INTERNAL_SERVER_ERROR).is_unauthorized());
        // A 401 wrapped by the retry layer is still recognised.
        assert!(
            ApiError::RetriesExhausted {
                attempts: 3,
                last_error: Box::new(api(reqwest::StatusCode::UNAUTHORIZED)),
            }
            .is_unauthorized()
        );
        // A local "expired" signal is not a server 401 (different recovery).
        assert!(!ApiError::ExpiredOAuthToken.is_unauthorized());
    }

    #[test]
    fn is_rate_limit_detects_429_and_529_and_overload() {
        let api = |status, retry_after| ApiError::Api {
            status,
            error_type: None,
            message: None,
            body: String::new(),
            retryable: true,
            retry_after,
        };
        // 429 and 529 (Anthropic overload) are rate-limit signals.
        assert!(api(StatusCode::TOO_MANY_REQUESTS, None).is_rate_limit());
        assert!(api(StatusCode::from_u16(529).unwrap(), None).is_rate_limit());
        // A 401 / 500 is not a rate limit (must fail fast, not absorb).
        assert!(!api(StatusCode::UNAUTHORIZED, None).is_rate_limit());
        assert!(!api(StatusCode::INTERNAL_SERVER_ERROR, None).is_rate_limit());
        // Wrapped by the retry layer, the classification still holds.
        assert!(
            ApiError::RetriesExhausted {
                attempts: 3,
                last_error: Box::new(api(StatusCode::TOO_MANY_REQUESTS, None)),
            }
            .is_rate_limit()
        );
        // A streamed overload error is classified from its type/message.
        let stream_overload = ApiError::StreamApi {
            error_type: Some("overloaded_error".to_string()),
            message: Some("Overloaded".to_string()),
            body: String::new(),
            retryable: true,
        };
        assert!(stream_overload.is_rate_limit());
        let stream_other = ApiError::StreamApi {
            error_type: Some("invalid_request_error".to_string()),
            message: Some("bad input".to_string()),
            body: String::new(),
            retryable: false,
        };
        assert!(!stream_other.is_rate_limit());
    }

    /// A 413 is a BYTE-size rejection, and the only lever the stack has is to
    /// shrink the body — so it must classify as `ContextOverflow`, which is
    /// what drives `recover_provider_context_overflow` (compact, then re-send).
    /// It previously matched no classifier arm and fell through to
    /// `NonRetryable`, killing the turn with a raw transport error.
    #[test]
    fn payload_too_large_classifies_as_context_overflow_not_fatal() {
        let anthropic_413 = ApiError::Api {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            error_type: Some("request_too_large".to_string()),
            message: Some("Request exceeds the maximum size".to_string()),
            body: r#"{"type":"error","error":{"type":"request_too_large"}}"#.to_string(),
            retryable: false,
            retry_after: None,
        };
        assert_eq!(
            anthropic_413.provider_error_class(),
            ProviderErrorClass::ContextOverflow
        );
        // `is_retryable` must stay false: re-sending the identical body is
        // deterministic. Recovery happens a layer up, after compaction.
        assert!(!anthropic_413.is_retryable());
        assert!(!anthropic_413.is_rate_limit());

        // Status alone is enough — a gateway may word the body however it likes.
        let bare_413 = ApiError::Api {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            error_type: None,
            message: None,
            body: "<html>413 Request Entity Too Large</html>".to_string(),
            retryable: false,
            retry_after: None,
        };
        assert_eq!(
            bare_413.provider_error_class(),
            ProviderErrorClass::ContextOverflow
        );

        // The streamed path carries no status code, so it must be recognized
        // from wording alone.
        let streamed = ApiError::StreamApi {
            error_type: Some("request_too_large".to_string()),
            message: Some("Request exceeds the maximum size".to_string()),
            body: String::new(),
            retryable: false,
        };
        assert_eq!(
            streamed.provider_error_class(),
            ProviderErrorClass::ContextOverflow
        );

        // Neighbouring 4xx codes must NOT be swept into overflow recovery.
        let bad_request = ApiError::Api {
            status: StatusCode::BAD_REQUEST,
            error_type: Some("invalid_request_error".to_string()),
            message: Some("bad input".to_string()),
            body: String::new(),
            retryable: false,
            retry_after: None,
        };
        assert_eq!(
            bad_request.provider_error_class(),
            ProviderErrorClass::NonRetryable
        );
    }

    #[test]
    fn stream_overloaded_error_display_names_provider_capacity() {
        let error = ApiError::StreamApi {
            error_type: Some("overloaded_error".to_string()),
            message: Some("Overloaded".to_string()),
            body: String::new(),
            retryable: true,
        };
        let rendered = error.to_string();
        assert!(rendered.contains("Provider capacity issue"));
        // The message has to name the SCOPE, not just the symptom: the reflex on
        // reading it was "my rate limit", while the account window was 2 % used.
        assert!(rendered.contains("NOT your account's rate limit"));
        assert!(rendered.contains("workspace-trust failure"));
        assert!(error.is_retryable());
        assert!(error.provider_error_class().is_provider_overload());
    }

    #[test]
    fn unsupported_vision_error_display_appends_hint() {
        let stream_error = ApiError::StreamApi {
            error_type: Some("invalid_request_error".to_string()),
            message: Some("Model 'gpt-5.3-codex-spark' does not support image inputs.".to_string()),
            body: String::new(),
            retryable: false,
        };
        assert!(stream_error.is_unsupported_vision());
        let text = stream_error.to_string();
        assert!(text.contains("Model 'gpt-5.3-codex-spark' does not support image inputs."));
        assert!(text.contains(UNSUPPORTED_VISION_HINT_BODY));

        let api_error = ApiError::Api {
            status: StatusCode::BAD_REQUEST,
            error_type: Some("invalid_request_error".to_string()),
            message: Some("Model 'gpt-5.3-codex-spark' does not support image inputs.".to_string()),
            body: String::new(),
            retryable: false,
            retry_after: None,
        };
        assert!(api_error.is_unsupported_vision());
        let api_text = api_error.to_string();
        assert!(api_text.contains(UNSUPPORTED_VISION_HINT_BODY));
    }

    #[test]
    fn retry_after_is_surfaced_through_retry_exhaustion() {
        let inner = ApiError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: None,
            message: None,
            body: String::new(),
            retryable: true,
            retry_after: Some(Duration::from_secs(42)),
        };
        assert_eq!(inner.retry_after(), Some(Duration::from_secs(42)));
        let wrapped = ApiError::RetriesExhausted {
            attempts: 5,
            last_error: Box::new(inner),
        };
        assert_eq!(
            wrapped.retry_after(),
            Some(Duration::from_secs(42)),
            "Retry-After must survive the retry-exhaustion wrapper"
        );
    }

    fn api_error(
        status: StatusCode,
        error_type: Option<&str>,
        message: Option<&str>,
        body: &str,
        retryable: bool,
    ) -> ApiError {
        ApiError::Api {
            status,
            error_type: error_type.map(str::to_string),
            message: message.map(str::to_string),
            body: body.to_string(),
            retryable,
            retry_after: None,
        }
    }

    /// Both statuses are capacity refusals, but they name **different** windows:
    /// 429 is this account's, 529 is the provider's. Every downstream reaction
    /// (cool-down ladder, wall-clock budget, 95 %-utilization swap gate) branches
    /// on that scope, so the classifier must not collapse them.
    #[test]
    fn provider_error_scopes_429_to_the_account_and_529_to_the_provider() {
        let rate_limit = ApiError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: Some("rate_limit_error".to_string()),
            message: Some("slow down".to_string()),
            body: String::new(),
            retryable: true,
            retry_after: Some(Duration::from_secs(7)),
        };
        assert_eq!(
            rate_limit.provider_error_class(),
            ProviderErrorClass::account_rate_limit(Some(Duration::from_secs(7)))
        );
        assert_eq!(
            api_error(
                StatusCode::from_u16(529).unwrap(),
                Some("overloaded_error"),
                Some("busy"),
                "",
                true,
            )
            .provider_error_class(),
            ProviderErrorClass::provider_overloaded(None)
        );
        assert!(
            api_error(
                StatusCode::from_u16(529).unwrap(),
                Some("overloaded_error"),
                Some("busy"),
                "",
                true,
            )
            .provider_error_class()
            .is_provider_overload()
        );
    }

    /// Every human-facing hint must be invisible to the retry classifier.
    ///
    /// This is the regression the unit tests missed and a live-fire run caught: the
    /// 529 hint says "this is NOT your account's rate limit", the classifier
    /// matched `rate limit` inside that denial, and the whole scope split
    /// collapsed back to Account — 300 s of retries plus a 120 s account cool-down
    /// on a 2 %-utilized window. The retry-exhaustion hint ("Rate limited. Try: …")
    /// carried the same hazard before the split existed.
    ///
    /// Asserted over the FULL `Display` output of each hint-carrying error, since
    /// classifying the bare head is exactly what hid it.
    #[test]
    fn recovery_hints_never_change_the_retry_classification() {
        use core_types::retry_signal::{classify_error_text, RetrySignal};

        let streamed_overload = ApiError::StreamApi {
            error_type: Some("overloaded_error".to_string()),
            message: Some("Overloaded".to_string()),
            body: String::new(),
            retryable: true,
        };
        let status_overload = api_error(
            StatusCode::from_u16(529).unwrap(),
            Some("overloaded_error"),
            Some("Overloaded"),
            "",
            true,
        );
        let exhausted_overload = ApiError::RetriesExhausted {
            attempts: 6,
            last_error: Box::new(api_error(
                StatusCode::from_u16(529).unwrap(),
                Some("overloaded_error"),
                Some("Overloaded"),
                "",
                true,
            )),
        };
        for error in [&streamed_overload, &status_overload, &exhausted_overload] {
            let rendered = error.to_string();
            assert!(
                rendered.contains(HINT_SEPARATOR),
                "this fixture is supposed to carry a hint: {rendered}"
            );
            assert_eq!(
                classify_error_text(&rendered),
                RetrySignal::Overloaded,
                "a hint must not turn a provider overload into an account throttle: {rendered}"
            );
        }

        // The account side must stay Account with its own hint attached, so the
        // separator is not silently swallowing the real signal either.
        let throttled = ApiError::RetriesExhausted {
            attempts: 6,
            last_error: Box::new(api_error(
                StatusCode::TOO_MANY_REQUESTS,
                Some("rate_limit_error"),
                Some("This request would exceed your account's rate limit"),
                "",
                true,
            )),
        };
        assert_eq!(
            classify_error_text(&throttled.to_string()),
            RetrySignal::RateLimit
        );

        // And a hint-carrying auth failure stays fatal rather than being read as
        // capacity because its prose mentions retrying.
        let unauthorized = api_error(StatusCode::UNAUTHORIZED, None, None, "", false);
        assert_eq!(
            classify_error_text(&unauthorized.to_string()),
            RetrySignal::Fatal
        );
    }

    /// The reported failure: an `overloaded_error` SSE frame on an HTTP 200
    /// stream, where there is no status to read and the frame's own type is the
    /// only scope evidence. Mis-scoping it to the account is what let the
    /// 95 %-utilization gate refuse the model swap that recovers the turn.
    #[test]
    fn streamed_overload_frame_scopes_to_the_provider() {
        let streamed = ApiError::StreamApi {
            error_type: Some("overloaded_error".to_string()),
            message: Some("Overloaded".to_string()),
            body: r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
                .to_string(),
            retryable: true,
        };
        assert_eq!(
            streamed.provider_error_class(),
            ProviderErrorClass::provider_overloaded(None)
        );
        // Still a capacity signal for everything that only asks that much.
        assert!(streamed.is_rate_limit());

        let streamed_429 = ApiError::StreamApi {
            error_type: Some("rate_limit_error".to_string()),
            message: Some("This request would exceed your account's rate limit".to_string()),
            body: String::new(),
            retryable: true,
        };
        assert_eq!(
            streamed_429.provider_error_class(),
            ProviderErrorClass::account_rate_limit(None)
        );
    }

    #[test]
    fn provider_error_class_preserves_retry_after_through_retries_exhausted() {
        let wrapped = ApiError::RetriesExhausted {
            attempts: 3,
            last_error: Box::new(ApiError::Api {
                status: StatusCode::TOO_MANY_REQUESTS,
                error_type: None,
                message: None,
                body: String::new(),
                retryable: true,
                retry_after: Some(Duration::from_secs(42)),
            }),
        };
        assert_eq!(
            wrapped.provider_error_class(),
            ProviderErrorClass::account_rate_limit(Some(Duration::from_secs(42)))
        );
    }

    /// A usage limit says what it is and when it lifts, in clock words, after
    /// the hint separator — and its head keeps the seconds for the retry layers.
    #[test]
    fn a_usage_limit_names_its_reset_in_clock_words_after_the_hint_separator() {
        let error = ApiError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: Some("usage_limit_reached".to_string()),
            message: Some("The usage limit has been reached".to_string()),
            body: String::new(),
            retryable: false,
            retry_after: Some(Duration::from_secs(8220)),
        };
        let text = error.to_string();
        let (head, hint) = text.split_once(HINT_SEPARATOR).expect("a hint follows the head");
        assert!(head.ends_with("retry-after: 8220"), "head: {head}");
        assert!(hint.contains("resets in 2h 17m"), "hint: {hint}");
        assert!(hint.contains("usage limit"), "hint: {hint}");
        assert_eq!(
            error.provider_error_class(),
            ProviderErrorClass::account_rate_limit(Some(Duration::from_secs(8220)))
        );

        let unknown = ApiError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            error_type: Some("usage_limit_reached".to_string()),
            message: Some("The usage limit has been reached".to_string()),
            body: String::new(),
            retryable: false,
            retry_after: None,
        };
        let text = unknown.to_string();
        let (_, hint) = text.split_once(HINT_SEPARATOR).expect("a hint follows the head");
        assert!(hint.contains("when its window rolls over"), "hint: {hint}");
    }

    #[test]
    fn provider_error_classifies_401_as_auth_expired() {
        assert_eq!(
            api_error(StatusCode::UNAUTHORIZED, None, None, "", false).provider_error_class(),
            ProviderErrorClass::AuthExpired
        );
    }

    #[test]
    fn client_rejection_401_is_not_auth_expired_and_not_unauthorized() {
        // agentrouter-style whitelist rejection: the token is valid, but the
        // provider refuses zo as an unauthorized client. This must not drive
        // the OAuth-refresh recovery path (which would 401 identically forever),
        // and must not be classified as an expired credential.
        let err = api_error(
            StatusCode::UNAUTHORIZED,
            Some("unauthorized_client_error"),
            Some("unauthorized client detected, contact support for assistance"),
            r#"{"type":"unauthorized_client_error","message":"UNAUTHENTICATED"}"#,
            false,
        );
        assert_eq!(
            err.provider_error_class(),
            ProviderErrorClass::NonRetryable,
            "client-rejection 401 must fail fast, not route through auth recovery"
        );
        assert!(
            !err.is_unauthorized(),
            "client-rejection 401 must not trigger the one-shot OAuth refresh + retry"
        );
        let rendered = err.to_string();
        assert!(
            rendered.contains("unauthorized client")
                && rendered.contains("Re-running /login will not help"),
            "message must explain it is a client rejection, not a credential expiry: {rendered}"
        );
        assert!(
            !rendered.contains("zo login [provider]"),
            "must not send the user into a futile re-login loop: {rendered}"
        );
        assert!(
            rendered.contains("client_fingerprint") && rendered.contains("codex"),
            "message must point at the actual fix — a client_fingerprint preset — not a dead end: {rendered}"
        );
    }

    #[test]
    fn plain_401_still_drives_reauth() {
        // A genuine credential 401 (no client-rejection signal) is unchanged:
        // classified as AuthExpired, recognised by is_unauthorized, and shown
        // with the re-login hint.
        let err = api_error(
            StatusCode::UNAUTHORIZED,
            Some("authentication_error"),
            Some("Invalid authentication credentials"),
            "",
            false,
        );
        assert_eq!(err.provider_error_class(), ProviderErrorClass::AuthExpired);
        assert!(err.is_unauthorized());
        assert!(err.to_string().contains("zo login [provider]"));
    }

    #[test]
    fn provider_error_classifies_transient_http_and_5xx() {
        assert_eq!(
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                Some("api_error"),
                Some("try later"),
                "",
                true,
            )
            .provider_error_class(),
            ProviderErrorClass::Transient
        );
        assert_eq!(
            api_error(StatusCode::REQUEST_TIMEOUT, None, None, "", true).provider_error_class(),
            ProviderErrorClass::Transient
        );
    }

    #[test]
    fn provider_error_classifies_context_overflow() {
        assert_eq!(
            api_error(
                StatusCode::BAD_REQUEST,
                Some("invalid_request_error"),
                Some("context length exceeded"),
                "maximum context window exceeded",
                false,
            )
            .provider_error_class(),
            ProviderErrorClass::ContextOverflow
        );
    }

    /// Anthropic's *actual* context-overflow wording, verbatim from a live 400.
    ///
    /// The test above pinned invented phrasing ("context length exceeded",
    /// "maximum context window exceeded") that Anthropic never sends, so the one
    /// message the overflow recovery exists to handle fell through every branch
    /// to `NonRetryable`: no compaction, no re-send, the turn just died. A
    /// sub-agent grinding through tool output hit it constantly while Claude Code
    /// compacted and carried on.
    #[test]
    fn anthropic_prompt_too_long_is_context_overflow() {
        let error = api_error(
            StatusCode::BAD_REQUEST,
            Some("invalid_request_error"),
            Some("prompt is too long: 211352 tokens > 200000 maximum"),
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 211352 tokens > 200000 maximum"}}"#,
            false,
        );
        assert_eq!(
            error.provider_error_class(),
            ProviderErrorClass::ContextOverflow,
            "the provider's own overflow message must reach the compact-and-resend path"
        );
    }

    /// The provider's stated ceiling is the only authority on what the wire
    /// accepts, so it has to be read exactly — and never guessed from an
    /// unrelated number in the message (the offending size is the *larger* one
    /// and sits on the other side of the `>`).
    #[test]
    fn the_stated_ceiling_is_read_from_the_overflow_message() {
        assert_eq!(
            context_overflow_ceiling_tokens("prompt is too long: 211352 tokens > 200000 maximum"),
            Some(200_000)
        );
        assert_eq!(
            api_error(
                StatusCode::BAD_REQUEST,
                Some("invalid_request_error"),
                Some("prompt is too long: 2082664 tokens > 200000 maximum"),
                "",
                false,
            )
            .context_overflow_ceiling(),
            Some(200_000)
        );
        // No ceiling stated → the caller keeps its own window rather than
        // adopting a number that was never claimed.
        assert_eq!(
            context_overflow_ceiling_tokens("context length exceeded"),
            None
        );
        assert_eq!(context_overflow_ceiling_tokens("a > b maximum"), None);
        assert_eq!(
            context_overflow_ceiling_tokens("saw 5 > 3 results"),
            None,
            "a bare comparison must not be mistaken for a token ceiling"
        );
        assert_eq!(context_overflow_ceiling_tokens("> 0 maximum"), None);
    }

    #[test]
    fn request_buffer_overflow_is_context_overflow_and_not_retryable() {
        let error = api_error(
            StatusCode::INSUFFICIENT_STORAGE,
            None,
            None,
            "exceeded request buffer limit while retrying upstream",
            true,
        );
        assert_eq!(
            error.provider_error_class(),
            ProviderErrorClass::ContextOverflow
        );
        assert!(!error.is_retryable());
    }

    #[test]
    fn provider_error_classifies_gemini_invalid_tool_schema_as_non_retryable() {
        assert_eq!(
            api_error(
                StatusCode::BAD_REQUEST,
                Some("INVALID_ARGUMENT"),
                Some("GenerateContentRequest.tools[0].function_declarations[0].parameters.properties: invalid schema"),
                "bad tool schema",
                false,
            )
            .provider_error_class(),
            ProviderErrorClass::InvalidToolSchema
        );
        assert!(
            !api_error(
                StatusCode::BAD_REQUEST,
                Some("INVALID_ARGUMENT"),
                Some("function_declarations parameters invalid schema"),
                "",
                false,
            )
            .is_retryable()
        );
    }

    #[test]
    fn provider_error_classifies_gemini_missing_thought_signature_as_invalid_tool_protocol() {
        assert_eq!(
            api_error(
                StatusCode::BAD_REQUEST,
                Some("INVALID_ARGUMENT"),
                Some("functionCall read_file is missing a thought_signature"),
                "Gemini tool protocol rejected the request",
                false,
            )
            .provider_error_class(),
            ProviderErrorClass::InvalidToolProtocol
        );
    }

    /// A gateway's content filter (`AgentRouter` and the relays built like it)
    /// refuses a request for its text: a class of its own, never retried as
    /// is — not even the 500 the filter answers with — while a model's own
    /// safety block and a plain 500 keep the classes they always had.
    #[test]
    fn a_gateway_content_refusal_is_its_own_class_and_never_retried() {
        for (status, error_type, message, retryable) in [
            (StatusCode::BAD_REQUEST, "agent_router_api_error", "content-blocked", false),
            (StatusCode::INTERNAL_SERVER_ERROR, "new_api_error", "sensitive_words_detected", true),
        ] {
            let error = api_error(status, Some(error_type), Some(message), "", retryable);
            assert_eq!(error.provider_error_class(), ProviderErrorClass::ContentRefused, "{message}");
            assert!(!error.is_retryable(), "{message} was retried as is");
        }
        assert_eq!(
            api_error(StatusCode::INTERNAL_SERVER_ERROR, Some("api_error"), Some("upstream failed"), "", true)
                .provider_error_class(),
            ProviderErrorClass::Transient
        );
        assert_eq!(
            ApiError::StreamApi {
                error_type: Some("safety_blocked".to_string()),
                message: Some("response blocked by safety policy".to_string()),
                body: String::new(),
                retryable: false,
            }
            .provider_error_class(),
            ProviderErrorClass::SafetyBlocked
        );
    }

    #[test]
    fn provider_error_classifies_safety_blocked() {
        assert_eq!(
            ApiError::StreamApi {
                error_type: Some("safety_blocked".to_string()),
                message: Some("response blocked by safety policy".to_string()),
                body: String::new(),
                retryable: false,
            }
            .provider_error_class(),
            ProviderErrorClass::SafetyBlocked
        );
    }

    #[test]
    fn provider_error_classifies_plain_400_as_non_retryable() {
        assert_eq!(
            api_error(
                StatusCode::BAD_REQUEST,
                Some("invalid_request_error"),
                Some("bad request"),
                "plain validation error",
                false,
            )
            .provider_error_class(),
            ProviderErrorClass::NonRetryable
        );
    }

    #[test]
    fn provider_error_classifies_401_as_auth_even_with_rate_limit_text() {
        assert_eq!(
            api_error(
                StatusCode::UNAUTHORIZED,
                Some("invalid_api_key"),
                Some("unauthorized; previous request mentioned rate limit"),
                "quota and rate limit diagnostics are not auth class",
                false,
            )
            .provider_error_class(),
            ProviderErrorClass::AuthExpired
        );
    }

    #[test]
    fn provider_error_class_does_not_treat_generic_schema_text_as_tool_schema() {
        assert_eq!(
            api_error(
                StatusCode::BAD_REQUEST,
                Some("invalid_request_error"),
                Some("response_format json schema is invalid"),
                "schema validation failed",
                false,
            )
            .provider_error_class(),
            ProviderErrorClass::NonRetryable
        );
        assert_eq!(
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                Some("server_error"),
                Some("internal invalid schema cache"),
                "generic schema cache failure",
                true,
            )
            .provider_error_class(),
            ProviderErrorClass::Transient
        );
    }
}
