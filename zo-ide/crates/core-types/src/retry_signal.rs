//! Single source of truth for classifying a stringified provider error into a
//! retry signal.
//!
//! Several layers independently decide "is this a transient capacity failure?"
//! from a flattened error string:
//!
//! - the `runtime` retry layer picks a backoff *schedule* (longer for capacity
//!   stalls so an overload retry doesn't hammer the pool),
//! - the `runtime` conversation layer picks a live-UI *label* ("provider
//!   overloaded" / "rate limited" / "transient provider error"),
//! - the `runtime` Anthropic stream parser decides whether a `Transport` error
//!   is a *provider-emitted* frame (server already closed the turn — surface it
//!   for a fresh request) versus a recoverable connection drop.
//!
//! Before this module each site carried its own `contains("…")` predicate, so a
//! new provider wording (e.g. a different overload phrase) had to be added in
//! every place or one layer would silently disagree with the others. Keeping the
//! substring vocabulary here means every layer classifies from the *same* words
//! and only maps the resulting [`RetrySignal`] to its own concern.
//!
//! This is a deliberately text-based classifier for the flattened-display path.
//! Callers that hold the *structured* error (e.g. `api::ApiError` with an HTTP
//! status code) should classify from the status directly; this module is the
//! shared fallback for the many places that only see a `Display` string.

use std::time::Duration;

/// How a stringified provider error should be treated by a retry layer.
///
/// Ordering is by escalation, not severity: a capacity stall
/// ([`RateLimit`](Self::RateLimit) / [`Overloaded`](Self::Overloaded)) is "more
/// transient" than a generic [`Transient`](Self::Transient) blip in that it
/// wants a *longer* backoff, while [`Fatal`](Self::Fatal) must fail fast
/// (auth / validation errors that retrying can never fix).
///
/// The two capacity variants exist because **whose** capacity ran out decides
/// what actually recovers the turn, and the two answers are opposites:
///
/// * [`RateLimit`](Self::RateLimit) — *this account's* window is squeezed
///   (HTTP 429 / `rate_limit_error`). Only time helps; the same request on a
///   different model of the same provider hits the same wall, so the honest
///   move is to ride the window out.
/// * [`Overloaded`](Self::Overloaded) — *the provider* shed the request
///   (HTTP 529 / `overloaded_error`). The account's window is irrelevant — this
///   fires at 2 % utilization — the window is seconds-to-minutes rather than the
///   plan's hours, and a lighter model or another provider usually succeeds
///   immediately. Riding it out on the same model is the one thing that does
///   not work.
///
/// Collapsing them into one signal is what let a five-second provider hiccup
/// spend a five-minute account-throttle budget and then refuse the model swap
/// that would have worked (the "hi 쳤는데 Overloaded" turn death).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrySignal {
    /// This account's rate-limit window is exhausted — HTTP 429, a
    /// `rate_limit_error`, or "too many requests" / "rate limit" wording.
    /// Retryable on the *longer* schedule, and only time lifts it.
    RateLimit,
    /// The provider shed the request for its own capacity — HTTP 529 or an
    /// `overloaded_error`. Retryable on the *longer* schedule, but briefly:
    /// switching to a lighter model or another provider is the real recovery.
    Overloaded,
    /// A transient non-capacity failure — a 5xx or a connection/timeout drop.
    /// Retryable on the *standard* backoff schedule.
    Transient,
    /// Not retryable — an auth, validation, or otherwise permanent error.
    Fatal,
}

impl RetrySignal {
    /// True when the signal warrants a retry at all (capacity or transient).
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::RateLimit | Self::Overloaded | Self::Transient)
    }

    /// True for any provider capacity signal (429 / 529 / overloaded / rate
    /// limit) — i.e. "back off on the longer schedule". Callers that must know
    /// *whose* capacity ran out use [`Self::is_rate_limit`] /
    /// [`Self::is_overloaded`] instead.
    #[must_use]
    pub const fn is_capacity(self) -> bool {
        matches!(self, Self::RateLimit | Self::Overloaded)
    }

    /// True only for an **account-window** stall (429). Deliberately false for
    /// [`Self::Overloaded`]: a provider overload must not stamp this account's
    /// throttle state or wait out its window.
    #[must_use]
    pub const fn is_rate_limit(self) -> bool {
        matches!(self, Self::RateLimit)
    }

    /// True only for a **provider-capacity** stall (529 / `overloaded_error`).
    #[must_use]
    pub const fn is_overloaded(self) -> bool {
        matches!(self, Self::Overloaded)
    }
}

/// True when the lowercased error text says **this account's** window is
/// squeezed: HTTP 429, or "rate limit" / "too many requests" wording.
///
/// Deliberately excludes 529 / "overloaded" — see [`is_overloaded_text`].
///
/// `lower` MUST already be ASCII-lowercased by the caller (see
/// [`classify_error_text`], which lowercases once and reuses the result).
#[must_use]
pub fn is_account_rate_limit_text(lower: &str) -> bool {
    lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("too many requests")
        || is_usage_limit_text(lower)
}

/// True when the lowercased error text names an exhausted **plan window** —
/// ChatGPT's `usage_limit_reached` / "The usage limit has been reached". A
/// usage limit is this account's window (see [`is_account_rate_limit_text`],
/// which includes it); it is singled out here because its wording carried no
/// status and none of the rate-limit words, so it read as a transient blip —
/// seven reconnects and a dead turn for a wall only the clock, or another
/// provider, lifts.
///
/// `lower` MUST already be ASCII-lowercased by the caller.
#[must_use]
pub fn is_usage_limit_text(lower: &str) -> bool {
    lower.contains("usage_limit") || lower.contains("usage limit")
}

/// True when the lowercased error text says **the provider** shed the request
/// for its own capacity: HTTP 529 or an `overloaded_error` / "overloaded".
///
/// `lower` MUST already be ASCII-lowercased by the caller.
#[must_use]
pub fn is_overloaded_text(lower: &str) -> bool {
    lower.contains("529") || lower.contains("overloaded")
}

/// True when the lowercased error text carries *either* capacity signal — the
/// umbrella predicate for "back off longer / this is not a hard failure".
///
/// Use this where only "is it capacity?" matters (backoff schedule, provider-
/// emitted-frame detection). Where the *scope* changes the recovery, classify
/// with [`classify_error_text`] and branch on the variant instead.
///
/// `lower` MUST already be ASCII-lowercased by the caller.
#[must_use]
pub fn is_capacity_text(lower: &str) -> bool {
    is_account_rate_limit_text(lower) || is_overloaded_text(lower)
}

/// True when a lowercased error says the request BODY was too big to accept —
/// the provider's own 413 (`request_too_large` / "Request exceeds the maximum
/// size"), a gateway's 413 ("payload too large", "request entity too large"),
/// or the 507 case where an upstream retry gateway could not buffer the body.
/// Re-sending that same body is deterministic; callers must reduce it before
/// retrying.
///
/// This is a BYTE-size signal, not a token-count one. A request can sit well
/// inside the model's context window and still be refused — many images, large
/// base64 attachments, or a few huge tool results blow the body limit long
/// before the token ceiling. It is nevertheless classified alongside the
/// token-overflow signals because the recovery is identical (compact, then
/// retry) and compaction is the only lever the stack has for shrinking a
/// request.
#[must_use]
pub fn is_request_too_large_text(lower: &str) -> bool {
    (lower.contains("request buffer limit")
        && (lower.contains("exceeded") || lower.contains("exceeds")))
        || lower.contains("request_too_large")
        || lower.contains("request too large")
        || lower.contains("payload too large")
        || lower.contains("request entity too large")
        || lower.contains("exceeds the maximum size")
}

/// A gateway's content filter refused the request for its text — the relays
/// built on new-api (`AgentRouter` among them) answer `content-blocked` (400) or
/// `sensitive_words_detected` (500) when their filter scores the prompt as
/// unwanted. The same bytes are refused again, so it is never retried as is;
/// what can change the verdict is the text, which the runtime may trim of what
/// it attached on its own before asking once more. No first-party provider
/// answers with these words.
#[must_use]
pub fn is_content_refusal_text(lower: &str) -> bool {
    lower.contains("content-blocked")
        || lower.contains("content_blocked")
        || lower.contains("sensitive_words_detected")
}

/// Recovery advice appended to a model capability refusal when image inputs
/// are rejected by a text-only endpoint.
pub const UNSUPPORTED_VISION_HINT_BODY: &str =
    "This model does not accept image inputs. Switch to a vision-capable model (e.g. /model gpt-5.6-sol) or remove the image attachment.";

/// True when a lowercased error text indicates the model does not accept image
/// (vision) inputs.
///
/// `lower` MUST already be ASCII-lowercased by the caller.
#[must_use]
pub fn is_unsupported_vision_text(lower: &str) -> bool {
    lower.contains("does not support image inputs")
        || lower.contains("does not support image input")
        || lower.contains("does not accept image")
        || lower.contains("does not accept images")
        || lower.contains("unknown variant image_url")
}

/// The host a request never reached, when the error text says the NETWORK
/// failed rather than the provider: reqwest's `error sending request for url
/// (…)` and `error trying to connect`, a DNS failure, a route that is gone.
/// A refused connection is deliberately not one — the network carried the
/// SYN and the far side said no — and neither is a timeout, which a slow
/// backend produces as readily as a dead link.
///
/// `None` for everything else. The host is what a caller probes to learn the
/// link is back.
/// The token an error head carries when the provider named its reset:
/// `; retry-after: <seconds>`. Written by the `api` error display and read
/// back by [`reset_hint_in_text`], so the seconds survive the flattening every
/// retry layer classifies from.
pub const RESET_HINT_TOKEN: &str = "retry-after: ";

/// The reset a flattened error names, if any — the seconds after
/// [`RESET_HINT_TOKEN`] in the machine-readable head. A hint past the hint
/// separator is prose and is not read.
#[must_use]
pub fn reset_hint_in_text(error_message: &str) -> Option<Duration> {
    let head = machine_readable_head(error_message);
    let (_, rest) = head.split_once(RESET_HINT_TOKEN)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u64>().ok().map(Duration::from_secs)
}

/// A reset wait the way a person reads a clock: `2h 17m`, `17m`, `40s`.
/// Minutes round up so a wait never reads shorter than it is.
#[must_use]
pub fn human_reset_wait(wait: Duration) -> String {
    let secs = wait.as_secs();
    if secs < 60 {
        return format!("{}s", secs.max(1));
    }
    let minutes = secs.div_ceil(60);
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
}

#[must_use]
pub fn network_outage_host(message: &str) -> Option<String> {
    let lower = message.to_ascii_lowercase();
    let outage = lower.contains("error sending request")
        || lower.contains("error trying to connect")
        || lower.contains("dns error")
        || lower.contains("failed to lookup address")
        || lower.contains("network is unreachable")
        || lower.contains("no route to host")
        || lower.contains("network is down");
    if !outage {
        return None;
    }
    let host = message
        .split_once("://")
        .map(|(_, rest)| rest)
        .map(|rest| rest.split(['/', ')', ' ', '?', '"']).next().unwrap_or(""))
        .map(|authority| authority.rsplit('@').next().unwrap_or(authority))
        .map(|host_port| host_port.split(':').next().unwrap_or(host_port))
        .filter(|host| !host.is_empty())
        .map(str::to_string);
    Some(host.unwrap_or_default())
}

/// True when the lowercased error text carries a *non-capacity* transient
/// signal: a retryable 5xx status or a connection/timeout/EOF drop.
///
/// The structured API layer treats every HTTP status `>= 500` as retryable.
/// This flattened-text fallback cannot safely parse arbitrary status tokens, so
/// keep the common provider/gateway server-error vocabulary explicit, including
/// Cloudflare-style 52x errors surfaced as `api returned 520 <unknown status code>`.
///
/// `lower` MUST already be ASCII-lowercased by the caller.
#[must_use]
pub fn is_transient_text(lower: &str) -> bool {
    lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("504")
        || lower.contains("520")
        || lower.contains("521")
        || lower.contains("522")
        || lower.contains("523")
        || lower.contains("524")
        || lower.contains("525")
        || lower.contains("526")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("connection closed")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("broken pipe")
        || lower.contains("eof")
        // reqwest/hyper mid-body drops. `error decoding response body` is
        // reqwest's Decode error — for a streaming SSE body it means the
        // connection died mid-transfer (truncated chunked/compressed stream),
        // the same class as `connection reset`, but its text matched nothing
        // here, so the turn failed fatally instead of retrying (the reported
        // "✘ turn: … error decoding response body"). `incomplete message` is
        // hyper's IncompleteMessage (server closed mid-response); `error
        // reading a body` covers hyper's body-read transport variant.
        || lower.contains("error decoding response body")
        || lower.contains("incomplete message")
        || lower.contains("error reading a body")
        // Some provider backends close an accepted streaming request with this
        // generic terminal marker and no structured status. A fresh request is
        // safe; treating it as fatal forced the user to submit `continue`.
        || lower.contains("backend reported a terminal stream failure")
}

/// Marker that separates the machine-readable head of an error string from the
/// human-facing recovery hint appended after it: a blank line plus a two-space
/// indent.
///
/// Load-bearing in **both** directions. `api::ApiError`'s `Display` emits every
/// hint with this exact prefix, and [`classify_error_text`] classifies only the
/// text before the first occurrence — because a hint is prose written for a
/// person and inevitably contains the very vocabulary this module matches on.
/// That is not hypothetical: the 529 hint explains "this is NOT your account's
/// rate limit", and matching it re-read a provider overload as an account
/// throttle, restoring the 300 s budget and the 120 s account cool-down the
/// scope split exists to prevent (caught by live-firing the real binary, not by
/// the unit tests, which classified the bare head). The retry-exhaustion hint
/// ("Rate limited. Try: …") carried the same hazard before that.
pub const HINT_SEPARATOR: &str = "\n\n  ";

/// The machine-readable head of an error string: everything before the first
/// [`HINT_SEPARATOR`]. Callers that match error *vocabulary* must classify this,
/// never the whole `Display` output.
#[must_use]
pub fn machine_readable_head(error_message: &str) -> &str {
    error_message
        .split_once(HINT_SEPARATOR)
        .map_or(error_message, |(head, _)| head)
}

/// Classify a raw (not yet lowercased) provider error string into a
/// [`RetrySignal`]. Strips any human-facing hint ([`HINT_SEPARATOR`]),
/// lowercases once, then applies the shared vocabulary: capacity signals win
/// over generic transient signals, and anything matching neither is
/// [`RetrySignal::Fatal`].
///
/// Within the capacity arms the **account** window wins over the overload
/// wording when a message somehow carries both. That direction is deliberate:
/// mis-reading a real 429 as an overload would swap models on an exhausted
/// account (the swap target shares the account and 429s too), whereas the
/// reverse mistake only costs one extra same-model retry.
#[must_use]
pub fn classify_error_text(error_message: &str) -> RetrySignal {
    let lower = machine_readable_head(error_message).to_ascii_lowercase();
    // A body that was refused for its SIZE — or for its TEXT by a gateway's
    // content filter — is deterministic: the same bytes will be refused again,
    // so this must never fall through to the transient or rate-limit arms and
    // burn a retry budget on an identical request (a request id's digits can
    // spell `520` and look transient). The structured API layer classifies them
    // as `ContextOverflow` / `ContentRefused` and changes the request before
    // re-sending; this flattened-text path has no such lever, so `Fatal` is the
    // honest answer.
    if is_request_too_large_text(&lower) || is_content_refusal_text(&lower) {
        RetrySignal::Fatal
    } else if is_account_rate_limit_text(&lower) {
        RetrySignal::RateLimit
    } else if is_overloaded_text(&lower) {
        RetrySignal::Overloaded
    } else if is_transient_text(&lower) {
        RetrySignal::Transient
    } else {
        RetrySignal::Fatal
    }
}

/// A provider stream *mid-flight* retry, surfaced so a live UI can show the
/// otherwise-silent reconnect pause as "reconnecting", not a freeze.
///
/// Distinct from the establish-time retry the `runtime` retry layer already
/// renders: this fires when a stream that has already started reading restarts
/// its own upstream connection internally (the provider closed the HTTP body
/// pre-commit). That internal restart never returns an error to the runtime
/// retry layer, so without this notice the turn just stalls for the backoff
/// delay. Carries the already-classified [`retry_notice_label`] text plus the
/// attempt counter and backoff delay so the consumer can render a message
/// without re-parsing the error.
#[derive(Debug, Clone)]
pub struct StreamRetryNotice {
    /// What this notice describes; drives the consumer's wording.
    pub kind: StreamNoticeKind,
    /// Human-readable cause label from [`retry_notice_label`].
    pub label: &'static str,
    /// 1-based restart attempt number.
    pub attempt: u32,
    /// Maximum attempts the stream will make before surfacing a hard error.
    pub max_attempts: u32,
    /// Backoff delay before this attempt's reconnect — or, for
    /// [`StreamNoticeKind::QuietReasoning`], how long the stream has been
    /// quiet so far.
    pub delay: std::time::Duration,
}

/// What a [`StreamRetryNotice`] describes: a reconnect backoff (the stream
/// stalled and is re-opening), or a quiet-reasoning heartbeat (the connection
/// is alive and delivering keep-alive chunks, but the model has produced no
/// visible event yet — deep reasoning on a large context can stay silent for
/// minutes, and without this signal it reads as a hang).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamNoticeKind {
    #[default]
    Reconnect,
    QuietReasoning,
}

/// Label of the quiet-reasoning heartbeat notice. A shared constant because
/// two layers key on the exact wording: the streaming backend stamps it on the
/// [`StreamNoticeKind::QuietReasoning`] notice, and the TUI matches the
/// transcript row's prefix to flip its "no output" stall badge into a calm
/// "reasoning · stream alive" state (the `STEERING_ECHO_PREFIX` precedent —
/// keep every emitter and matcher on this constant).
pub const QUIET_REASONING_LABEL: &str = "model reasoning silently — stream alive";

/// Prefix of the quota-hold warning the runtime emits when a hard 429 parks
/// the turn on the same model until its quota window resets (the wait band).
/// Shared for the same reason as [`QUIET_REASONING_LABEL`]: the runtime stamps
/// it on the transcript warning, and the TUI matches the prefix to flip the
/// spinner into a "rate-limited · holding for quota reset" state — without it
/// the (up to 15-minute) sleep reads as a plain "no output" hang and users
/// Esc out of a turn that would have resumed on its own.
pub const QUOTA_HOLD_NOTICE_PREFIX: &str = "Main model rate-limited";

/// System warning when a safety-classifier refusal retries the current turn on
/// the catalog's same-provider fallback (Fable/Sonnet/Haiku → the Opus head).
/// A function, not a constant, because the target is a catalog fact: the
/// runtime never spells a release id, and this names the one it swapped to.
#[must_use]
pub fn refusal_fallback_warn(target: &str) -> String {
    format!(
        "The model's safety classifier declined this request — retrying on {target}."
    )
}

/// System warning when a refusal has been declined twice and the catalog's
/// refusal fallback is on another provider: this turn is handed to `target`
/// like a quota fallback, so the reply is not lost to a sticky classifier.
#[must_use]
pub fn refusal_cross_provider_warn(target: &str) -> String {
    format!(
        "The model's safety classifier declined this request twice — handing this turn to \
         {target} on another provider. The original model resumes automatically afterward."
    )
}

/// System notice for the P3 last resort: no fallback is available, so the turn
/// dropped the earlier declined exchange still in context and asked the same
/// model once more (the classifier reads the whole conversation).
pub const REFUSAL_CONTEXT_CLEANED_WARN: &str =
    "No provider fallback is available — asked the same model again with the earlier declined \
     exchange dropped from context.";

/// System warning on the first turn pre-armed by the session-scoped refusal
/// cooldown, naming the model the session now continues on (a same-provider
/// Opus override or a cross-provider peer) — or "the refusal fallback" when
/// none is known yet. A function so the two emitters (sync eprintln,
/// streaming render block) share one wording, and so the threshold and the
/// cooldown are the runtime's own constants rather than numbers repeated in
/// prose: `turns` is the consecutive-refusal count that arms the cooldown and
/// `cooldown` its length.
#[must_use]
pub fn refusal_prearm_warn(model: Option<&str>, turns: u8, cooldown: std::time::Duration) -> String {
    let target = match model {
        Some(model) if !model.trim().is_empty() => model,
        _ => "the refusal fallback",
    };
    let minutes = cooldown.as_secs().div_ceil(60);
    format!(
        "Safety classifier declined {turns} consecutive turns — continuing this session on {target} \
         for ~{minutes}m; the original model retries automatically afterward."
    )
}

/// Prefix of both quota-fallback notices. The fallback model id follows this
/// prefix and ends at the first `;`, allowing renderers to distinguish an
/// active provider swap from [`QUOTA_HOLD_NOTICE_PREFIX`] without parsing the
/// rest of the human-readable explanation.
pub const QUOTA_FALLBACK_ACTIVE_NOTICE_PREFIX: &str = "Quota fallback active on ";

/// Extract the active fallback model from a quota-fallback system notice.
///
/// Returns `None` for quota-hold notices and malformed or empty fallback
/// markers. The returned slice borrows the model id directly from `text`.
#[must_use]
pub fn parse_quota_fallback_model(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(QUOTA_FALLBACK_ACTIVE_NOTICE_PREFIX)?;
    let (model, _) = rest.split_once(';')?;
    (!model.is_empty()).then_some(model)
}

/// A short, human-readable label for a stringified provider error, for a live
/// "retrying in Ns" notice. Distinguishes an explicit *overload* from a plain
/// rate limit so the user can see which capacity wall they hit, and keeps that
/// wording in the same file as the classifier so it never drifts out of sync.
#[must_use]
pub fn retry_notice_label(error_message: &str) -> &'static str {
    match classify_error_text(error_message) {
        RetrySignal::Overloaded => "provider overloaded",
        RetrySignal::RateLimit => "rate limited",
        // A fatal error never reaches a "retrying in Ns" row, so the remaining
        // wording only has to cover the retryable non-capacity case. Deriving
        // every label from the one classifier is what keeps the notice honest:
        // the old independent `contains("overloaded")` here disagreed with the
        // backoff arm the moment either list changed.
        RetrySignal::Transient | RetrySignal::Fatal => "transient provider error",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        refusal_cross_provider_warn, refusal_fallback_warn, refusal_prearm_warn,
        QUOTA_FALLBACK_ACTIVE_NOTICE_PREFIX, QUOTA_HOLD_NOTICE_PREFIX,
        RetrySignal, classify_error_text, network_outage_host,
        parse_quota_fallback_model,
    };
    use std::time::Duration;

    /// A gateway's content filter refuses the same bytes every time, so its
    /// refusal is never retried as is — whatever its status (`AgentRouter`
    /// answers 400 `content-blocked` and 500 `sensitive_words_detected`) and
    /// whatever digits its request id happens to carry. A plain 500 without
    /// that vocabulary is still the transient it always was.
    #[test]
    fn a_gateway_content_refusal_is_fatal_whatever_its_status_or_request_id() {
        for refusal in [
            "provider transport: api returned 400 Bad Request (agent_router_api_error): content-blocked (request id: 20260912095204573)",
            "api returned 500 Internal Server Error (new_api_error): sensitive_words_detected",
        ] {
            assert_eq!(classify_error_text(refusal), RetrySignal::Fatal, "{refusal}");
        }
        assert_eq!(
            classify_error_text("api returned 500 Internal Server Error (api_error): upstream failed"),
            RetrySignal::Transient
        );
    }

    /// A request that never left the machine names the host it was for; a
    /// provider that answered, a refusal, or a timeout is not an outage.
    #[test]
    fn a_network_outage_is_told_from_a_provider_fault_and_names_its_host() {
        assert_eq!(
            network_outage_host(
                "provider stream: transport error sending request for url \
                 (https://chatgpt.com/backend-api/codex/responses): client error"
            )
            .as_deref(),
            Some("chatgpt.com")
        );
        assert_eq!(
            network_outage_host("error trying to connect: dns error: failed to lookup address").as_deref(),
            Some("")
        );
        assert_eq!(network_outage_host("api returned 503 overloaded"), None);
        assert_eq!(network_outage_host("connection refused (os error 61)"), None);
        assert_eq!(network_outage_host("operation timed out"), None);
    }

    /// An **account-window** stall: only time lifts it, so it keeps the long
    /// ride-it-out budget and must never trigger a same-account model swap.
    #[test]
    fn account_window_signals_classify_as_rate_limit() {
        for msg in [
            "HTTP 429 Too Many Requests",
            "rate_limit_error: rate limit exceeded",
            "Too Many Requests",
            "api returned 429 Too Many Requests (rate_limit_error): This request would exceed your account's rate limit.",
            "api stream error (usage_limit_reached): The usage limit has been reached",
        ] {
            assert_eq!(
                classify_error_text(msg),
                RetrySignal::RateLimit,
                "{msg:?} must be an account rate-limit signal"
            );
            assert!(classify_error_text(msg).is_capacity());
        }
    }

    /// A **provider-capacity** stall: the account window is irrelevant (this
    /// fires at 2 % utilization), so it gets its own signal and, downstream, its
    /// own short budget plus an immediate escape to a lighter model.
    #[test]
    fn provider_capacity_signals_classify_as_overloaded() {
        for msg in [
            "overloaded_error: Overloaded",
            "upstream OVERLOADED",
            "api stream error (529)",
            "api stream error (overloaded_error): Overloaded",
            "api returned 529 <unknown status code> (overloaded_error): Overloaded",
        ] {
            assert_eq!(
                classify_error_text(msg),
                RetrySignal::Overloaded,
                "{msg:?} must be a provider-overload signal"
            );
            let signal = classify_error_text(msg);
            assert!(signal.is_capacity(), "{msg:?} must back off on the long schedule");
            assert!(
                !signal.is_rate_limit(),
                "{msg:?} must not be charged to this account's window"
            );
        }
    }

    /// Both vocabularies in one string resolves to the account window: swapping
    /// models on an exhausted account only moves the 429 to the swap target.
    #[test]
    fn account_window_wins_when_both_vocabularies_appear() {
        assert_eq!(
            classify_error_text("api returned 429 Too Many Requests: upstream overloaded"),
            RetrySignal::RateLimit
        );
    }

    /// A body refused for its SIZE is deterministic — the same bytes fail
    /// again — so it must never reach the rate-limit or transient arms and
    /// spend a retry budget on an identical request.
    #[test]
    fn oversized_request_bodies_classify_as_fatal() {
        for msg in [
            "api returned 413 Payload Too Large (request_too_large): Request exceeds the maximum size",
            "413 Request Entity Too Large",
            "507 Insufficient Storage: request buffer limit exceeded",
            "payload too large",
        ] {
            assert_eq!(
                classify_error_text(msg),
                RetrySignal::Fatal,
                "{msg:?} must not be retried unchanged"
            );
        }
    }

    #[test]
    fn non_capacity_transient_signals_classify_as_transient() {
        for msg in [
            "HTTP 500 Internal Server Error",
            "502 Bad Gateway",
            "503 Service Unavailable",
            "504 Gateway Timeout",
            "api returned 520 <unknown status code>: error code: 520",
            "Cloudflare 522 connection timed out",
            "request timed out",
            "connection reset by peer",
            "broken pipe",
            // A streaming body that dies mid-transfer surfaces through
            // reqwest/hyper as a decode/read failure, not a named
            // connection error — it must retry, not kill the turn.
            "transport error: http error: error decoding response body",
            "connection error: incomplete message",
            "error reading a body from connection",
            "turn: runtime: provider stream: transport error: api stream error: backend reported a terminal stream failure",
        ] {
            assert_eq!(
                classify_error_text(msg),
                RetrySignal::Transient,
                "{msg:?} must be a transient signal"
            );
        }
    }

    #[test]
    fn permanent_errors_classify_as_fatal() {
        for msg in [
            "authentication_error: invalid API key",
            "invalid_request_error: messages too long",
            "401 Unauthorized",
            "507 Insufficient Storage: exceeded request buffer limit while retrying upstream",
        ] {
            assert_eq!(
                classify_error_text(msg),
                RetrySignal::Fatal,
                "{msg:?} must be fatal (fail fast)"
            );
        }
    }

    /// A human-facing hint appended after [`HINT_SEPARATOR`] must not reach the
    /// vocabulary match. The hint that broke this said "this is NOT your account's
    /// rate limit" — prose denying the account scope, from which the classifier
    /// derived the account scope.
    #[test]
    fn hints_after_the_separator_are_not_classified() {
        use super::{machine_readable_head, HINT_SEPARATOR};

        let head = "api stream error (overloaded_error): Overloaded";
        let rendered = format!(
            "{head}{HINT_SEPARATOR}Provider capacity issue — this is NOT your account's rate limit."
        );
        assert_eq!(machine_readable_head(&rendered), head);
        assert_eq!(classify_error_text(&rendered), RetrySignal::Overloaded);

        // A string with no hint is returned whole — the split must not eat a
        // machine-readable body that happens to contain blank lines elsewhere.
        let bodied = "api returned 400: {\n\n\"error\": \"bad\"}";
        assert_eq!(machine_readable_head(bodied), bodied);

        // And the separator does not hide a REAL signal that sits in the head.
        let throttled = format!("api returned 429 Too Many Requests{HINT_SEPARATOR}Rate limited.");
        assert_eq!(classify_error_text(&throttled), RetrySignal::RateLimit);
    }

    #[test]
    fn capacity_wins_over_transient_when_both_present() {
        // A 529 alongside a 503-looking body is still a capacity stall.
        assert_eq!(
            classify_error_text("api stream error 529 (503 backend)"),
            RetrySignal::Overloaded
        );
    }

    #[test]
    fn retryability_helpers_agree_with_variant() {
        assert!(RetrySignal::RateLimit.is_retryable());
        assert!(RetrySignal::RateLimit.is_capacity());
        assert!(RetrySignal::RateLimit.is_rate_limit());
        assert!(!RetrySignal::RateLimit.is_overloaded());
        assert!(RetrySignal::Overloaded.is_retryable());
        assert!(RetrySignal::Overloaded.is_capacity());
        assert!(RetrySignal::Overloaded.is_overloaded());
        // The load-bearing asymmetry: an overload is capacity pressure but is
        // NOT this account's throttle, so every account-scoped consequence
        // (window wait, cool-down stamp, 95 %-utilization swap gate) skips it.
        assert!(!RetrySignal::Overloaded.is_rate_limit());
        assert!(RetrySignal::Transient.is_retryable());
        assert!(!RetrySignal::Transient.is_capacity());
        assert!(!RetrySignal::Transient.is_rate_limit());
        assert!(!RetrySignal::Fatal.is_retryable());
        assert!(!RetrySignal::Fatal.is_capacity());
        assert!(!RetrySignal::Fatal.is_rate_limit());
    }

    /// The seconds a provider named ride the machine head as
    /// `retry-after: N`; prose after the hint separator is never read as one.
    #[test]
    fn a_reset_hint_is_read_from_the_machine_head_only() {
        use super::{HINT_SEPARATOR, RESET_HINT_TOKEN, reset_hint_in_text};
        assert_eq!(
            reset_hint_in_text(
                "api returned 429 Too Many Requests (usage_limit_reached): The usage limit \
                 has been reached; retry-after: 8220"
            ),
            Some(Duration::from_secs(8220))
        );
        assert_eq!(
            reset_hint_in_text(&format!(
                "api returned 429{HINT_SEPARATOR}wait, {RESET_HINT_TOKEN}30"
            )),
            None,
            "a hint is prose"
        );
        assert_eq!(reset_hint_in_text("api returned 429; retry-after: soon"), None);
        assert_eq!(reset_hint_in_text("connection reset"), None);
    }

    /// A reset wait reads like a clock, and minutes round up.
    #[test]
    fn a_reset_wait_reads_like_a_clock() {
        use super::human_reset_wait;
        assert_eq!(human_reset_wait(Duration::from_secs(8220)), "2h 17m");
        assert_eq!(human_reset_wait(Duration::from_secs(3600)), "1h 0m");
        assert_eq!(human_reset_wait(Duration::from_secs(61)), "2m");
        assert_eq!(human_reset_wait(Duration::from_secs(40)), "40s");
        assert_eq!(human_reset_wait(Duration::ZERO), "1s");
    }

    #[test]
    fn notice_label_distinguishes_overload_from_rate_limit() {
        use super::retry_notice_label;
        assert_eq!(
            retry_notice_label("api stream error (overloaded_error): Overloaded"),
            "provider overloaded"
        );
        assert_eq!(
            retry_notice_label("HTTP 429 Too Many Requests"),
            "rate limited"
        );
        assert_eq!(retry_notice_label("rate_limit exceeded"), "rate limited");
        assert_eq!(
            retry_notice_label("connection reset by peer"),
            "transient provider error"
        );
    }

    #[test]
    fn fallback_notice_markers_are_stable_and_unambiguous() {
        let model = "openai:gpt-5.6-sol";
        let swap = format!(
            "{QUOTA_FALLBACK_ACTIVE_NOTICE_PREFIX}{model}; the main model is rate-limited"
        );
        let prearm = format!(
            "{QUOTA_FALLBACK_ACTIVE_NOTICE_PREFIX}{model}; the main model is still cooling down"
        );
        assert_eq!(parse_quota_fallback_model(&swap), Some(model));
        assert_eq!(parse_quota_fallback_model(&prearm), Some(model));

        let hold = format!("{QUOTA_HOLD_NOTICE_PREFIX} (claude-fable-5); holding this turn");
        assert_eq!(parse_quota_fallback_model(&hold), None);
        assert_eq!(parse_quota_fallback_model("Quota fallback active on ; malformed"), None);
        // The refusal notices name the model they swapped to, and never the
        // release id from code — the target is a catalog fact handed in.
        assert_eq!(
            refusal_fallback_warn("claude-opus-5"),
            "The model's safety classifier declined this request — retrying on claude-opus-5."
        );
        assert!(refusal_cross_provider_warn("gpt-5.6-sol").contains("gpt-5.6-sol on another provider"));
        assert!(refusal_prearm_warn(Some("gpt-5.6-sol"), 2, std::time::Duration::from_secs(30 * 60)).contains("continuing this session on gpt-5.6-sol for ~30m"));
        // No model → the generic constant, which names no lineup.
        assert!(refusal_prearm_warn(None, 2, std::time::Duration::from_secs(90)).contains("2 consecutive turns — continuing this session on the refusal fallback for ~2m"));
        assert!(!refusal_prearm_warn(None, 2, std::time::Duration::from_secs(60)).contains("Fable"));
    }
}
