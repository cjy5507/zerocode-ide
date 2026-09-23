use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use core_types::{
    OAuthConfig, OAuthRefreshRequest, OAuthTokenExchangeRequest, RateLimitSnapshot,
    RateLimitWindow, RateLimitWindowKind, format_usd,
};

use crate::oauth_store::{load_oauth_credentials, save_oauth_credentials};
use serde::Deserialize;
use serde_json::{Map, Value};
use telemetry::{AnalyticsEvent, AnthropicRequestProfile, ClientIdentity, SessionTracer};

use super::{read_env_non_empty, refresh_gate};
use crate::credential::CredentialMiss;
use crate::error::ApiError;
use crate::prompt_cache::{PromptCache, PromptCacheRecord, PromptCacheStats};

use crate::sse::SseParser;
use crate::types::{
    ContextManagementResponse, MessageDeltaEvent, MessageRequest, MessageResponse,
    ReasoningRequest, StreamEvent, Usage,
};

/// Rewrite a request's thinking/effort shape to match what the target model's
/// Anthropic endpoint expects, returning a borrowed `Cow` (no clone) when the
/// request is already correct — the common legacy path.
///
/// Adaptive-thinking models (Opus 4.6+/Fable) take `output_config.effort` plus
/// `thinking:{type:"adaptive"}` and reject/ignore the deprecated
/// `thinking.budget_tokens`. So when such a model still carries a legacy budget
/// (or an explicit effort), we translate it: derive the effort (explicit
/// `request.effort` wins, else the budget maps via
/// [`super::effort_level_for_budget`]), set `output_config.effort`, and switch
/// thinking to adaptive. Legacy models and requests with no thinking are
/// returned untouched unless a pre-populated `output_config` needs a
/// provider-specific clamp.
fn normalize_thinking_for_wire(request: &MessageRequest) -> std::borrow::Cow<'_, MessageRequest> {
    use std::borrow::Cow;

    if !super::uses_adaptive_thinking(&request.model) {
        // `output_config` is not expected on legacy Anthropic models, but if a
        // caller pre-populates it, provider-neutral Ultra must still never
        // reach the Anthropic wire literally.
        if let Some(output_config) = request.output_config.as_ref() {
            let clamped = output_config.effort.anthropic_for_model(&request.model);
            if clamped != output_config.effort {
                let mut rewritten = request.clone();
                rewritten.output_config = Some(crate::types::OutputConfig::new(clamped));
                return Cow::Owned(rewritten);
            }
        }
        return Cow::Borrowed(request);
    }

    // Decide the effort for an adaptive model. Priority: an explicit
    // provider-neutral effort, else a legacy budget mapped to a level.
    //
    // `effort_band_ceiling` marks a dynamic band (Smart mode) rather than a
    // static pin: `level` is the floor (Xhigh), resolved per-request to a
    // concrete level BEFORE `anthropic_for_model` below. This is the only
    // way an escalated request ever reaches Anthropic's true `max` ceiling —
    // `anthropic_for_model` permanently clamps the provider-neutral `Ultra`
    // variant down to `Xhigh`, so the resolver must hand it a named `Max`
    // instead when a heavy turn escalates.
    let effort = match request.reasoning_request() {
        ReasoningRequest::Effort(level) => Some(match request.effort_band_ceiling {
            Some(ceiling) => super::resolve_effort_band(
                level,
                ceiling,
                &request.model,
                super::band_difficulty_for_request(request),
            ),
            None => level,
        }),
        ReasoningRequest::BudgetTokens(budget) => Some(super::effort_level_for_budget(budget)),
        ReasoningRequest::Auto => None,
    };

    let already_adaptive = request
        .thinking
        .as_ref()
        .is_none_or(|t| t.kind == "adaptive");
    // Nothing to translate: no effort to apply and thinking is already in the
    // adaptive shape (or absent) → leave the request untouched.
    if effort.is_none() && already_adaptive && request.output_config.is_none() {
        return Cow::Borrowed(request);
    }

    let mut rewritten = request.clone();
    if let Some(level) = effort {
        // Clamp the effort to what this specific model accepts before it reaches
        // the wire: adaptive Sonnet/Haiku reject `xhigh` (400 `This model does
        // not support effort level 'xhigh'`), so an inherited/demoted Xhigh
        // budget must drop to `high` rather than 400 the turn. Opus/Fable keep
        // the full scale. GPT fast mode is separate and does not clamp xhigh.
        let clamped = level.anthropic_for_model(&request.model);
        rewritten.output_config = Some(crate::types::OutputConfig::new(clamped));
        // Keep thinking enabled (adaptive) so the model still reasons; the budget
        // is now governed by effort, not an explicit token count.
        rewritten.thinking = Some(adaptive_thinking_for_model(&request.model));
    } else {
        // A caller may provide the Anthropic wire block directly without the
        // provider-neutral effort field. Clamp that path too: EffortLevel has a
        // provider-neutral `Ultra` variant, but Anthropic must never serialize it.
        if let Some(output_config) = rewritten.output_config.as_mut() {
            output_config.effort = output_config.effort.anthropic_for_model(&request.model);
        }
        if let Some(thinking) = rewritten.thinking.as_mut() {
            // No derived effort, but a legacy enabled-thinking block on an
            // adaptive model: drop its budget by switching to the adaptive shape.
            *thinking = adaptive_thinking_for_model(&request.model);
        }
    }
    Cow::Owned(rewritten)
}

/// Strip replayed reasoning blocks from the wire when this request has thinking
/// DISABLED.
///
/// `convert_blocks` lowers stored `Thinking`/`RedactedThinking` blocks
/// unconditionally — it runs before the effort/thinking budget is resolved — so a
/// turn recorded while thinking was on would otherwise be re-sent on a later
/// thinking-*off* request (e.g. after `/effort off`, `ZO_EFFORT=off`): an
/// assistant message carrying a `{"type":"thinking",…}` block with no `thinking`
/// config on the request. The API rejects that pairing (400), which then wedges
/// every subsequent turn because the same history re-sends the same block. When
/// thinking is enabled the blocks are valid (and must lead a `tool_use`), so they
/// are kept verbatim. Runs after [`normalize_thinking_for_wire`] so it sees the
/// final wire thinking state for every model (adaptive or legacy).
fn strip_thinking_blocks_when_disabled(
    request: std::borrow::Cow<'_, MessageRequest>,
) -> std::borrow::Cow<'_, MessageRequest> {
    // Fable and Mythos cannot turn thinking off, so a request to them with no
    // `thinking` field is still a thinking request and its replayed blocks are
    // valid — and on the 5.1 generation, taking them out would edit the
    // history every later block is bound to.
    if request.thinking.is_some() || super::thinking_always_on(&request.model) {
        return request;
    }
    let is_thinking_block = |block: &crate::types::InputContentBlock| {
        matches!(
            block,
            crate::types::InputContentBlock::Thinking { .. }
                | crate::types::InputContentBlock::RedactedThinking { .. }
        )
    };
    let carries_thinking = request
        .messages
        .iter()
        .any(|message| message.content.iter().any(is_thinking_block));
    if !carries_thinking {
        return request;
    }
    let mut owned = request.into_owned();
    for message in &mut owned.messages {
        message.content.retain(|block| !is_thinking_block(block));
    }
    // A reasoning-only assistant turn (a signed thinking block, or a
    // redacted_thinking block, with no following text/tool_use — reachable via a
    // max_tokens/refusal edge) is now empty. An empty-content message is itself a
    // 400, so drop it rather than trade one rejection for another. Safe: such a
    // turn carries no tool_use, so removing it orphans no tool_result.
    owned.messages.retain(|message| !message.content.is_empty());
    std::borrow::Cow::Owned(owned)
}

/// Beta that enables `thinking.block_binding` and adds `input_transformations`
/// to responses — preserved thinking's controls on Claude Fable 5.1 / Mythos
/// 5.1. First-party only: Bedrock and Vertex reject the header until they
/// carry the control.
const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// A forced tool choice never reaches a model that returns 400 for it.
///
/// Claude Fable 5.1 / Mythos 5.1: `tool_choice: type "tool" and "any" are not
/// supported for this model`. The runtime has already put the requirement in
/// the transcript (`force_structured_output_call`); this is the wire's own
/// guard, so no path — sub-agent, workflow, gateway — can 400 on it. `auto`
/// and `none` are unchanged.
fn normalize_tool_choice_for_wire(
    request: std::borrow::Cow<'_, MessageRequest>,
) -> std::borrow::Cow<'_, MessageRequest> {
    let forced = matches!(
        request.tool_choice,
        Some(crate::types::ToolChoice::Any | crate::types::ToolChoice::Tool { .. })
    );
    if !forced || !super::rejects_forced_tool_choice(&request.model) {
        return request;
    }
    let mut rewritten = request.into_owned();
    rewritten.tool_choice = Some(crate::types::ToolChoice::Auto);
    std::borrow::Cow::Owned(rewritten)
}

/// Preserved thinking on the 5.1 generation: bind the request's thinking so a
/// replayed block whose conversation prefix changed is dropped and reported
/// (`input_transformations`) rather than failing the request.
///
/// This harness does change prefixes — keep-tail compaction keeps recent turns
/// whose thinking was produced under the full history, and old tool results
/// are trimmed on the wire — so without this an enforced account would see
/// 400s on the turn after a compaction. `first_party` is false through a cloud
/// gateway, which rejects the beta header; the sender attaches the header
/// beside this field, never one without the other.
fn bind_thinking_for_wire(
    request: std::borrow::Cow<'_, MessageRequest>,
    first_party: bool,
) -> std::borrow::Cow<'_, MessageRequest> {
    if !first_party || !super::preserved_thinking_generation(&request.model) {
        return request;
    }
    let mut rewritten = request.into_owned();
    let thinking = rewritten
        .thinking
        .get_or_insert_with(|| adaptive_thinking_for_model(&rewritten.model));
    thinking.block_binding = Some(crate::types::BlockBinding::drop_block());
    std::borrow::Cow::Owned(rewritten)
}

/// The adaptive [`ThinkingConfig`] to send to an adaptive Anthropic `model`.
///
/// On models that accept the extended scale (Opus/Fable — same family gate as
/// `xhigh`), request `display:"summarized"` so the reasoning streams as visible
/// summary deltas. Without it, Opus 4.8's default `display:"omitted"` streams
/// empty thinking blocks and a long reasoning pass reads as a dead "no output"
/// pause. Sonnet/Haiku keep plain adaptive — their acceptance of `display` is
/// unconfirmed, so don't risk a 400 there.
fn adaptive_thinking_for_model(model: &str) -> crate::types::ThinkingConfig {
    if crate::providers::model_accepts_effort(model, crate::types::EffortLevel::Xhigh) {
        crate::types::ThinkingConfig::adaptive_summarized()
    } else {
        crate::types::ThinkingConfig::adaptive()
    }
}

pub mod keychain;

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const REQUEST_ID_HEADER: &str = "request-id";
const ALT_REQUEST_ID_HEADER: &str = "x-request-id";
const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(30);
const DEFAULT_MAX_RETRIES: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSource {
    None,
    ApiKey(String),
    BearerToken(String),
    ApiKeyAndBearer {
        api_key: String,
        bearer_token: String,
    },
}

impl AuthSource {
    pub fn from_env() -> Result<Self, ApiError> {
        let api_key = read_env_non_empty("ANTHROPIC_API_KEY")?;
        let auth_token = read_env_non_empty("ANTHROPIC_AUTH_TOKEN")?;
        match (api_key, auth_token) {
            (Some(api_key), Some(bearer_token)) => Ok(Self::ApiKeyAndBearer {
                api_key,
                bearer_token,
            }),
            (Some(api_key), None) => Ok(Self::ApiKey(api_key)),
            (None, Some(bearer_token)) => Ok(Self::BearerToken(bearer_token)),
            (None, None) => Err(ApiError::missing_credentials(
                "Anthropic",
                &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
            )),
        }
    }

    pub fn from_api_key_only() -> Result<Self, ApiError> {
        read_env_non_empty("ANTHROPIC_API_KEY")?
            .map(Self::ApiKey)
            .ok_or_else(|| ApiError::missing_auth_route_credentials("Anthropic", "api-key"))
    }

    pub fn from_oauth_only() -> Result<Self, ApiError> {
        if let Some(token) = read_env_non_empty("ANTHROPIC_AUTH_TOKEN")? {
            return Ok(Self::BearerToken(token));
        }
        resolve_claude_auth_fresh_inner()
            .map(|resolved| resolved.auth)
            .map_err(|_| ApiError::missing_auth_route_credentials("Anthropic", "oauth"))
    }

    #[must_use]
    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(api_key) | Self::ApiKeyAndBearer { api_key, .. } => Some(api_key),
            Self::None | Self::BearerToken(_) => None,
        }
    }

    #[must_use]
    pub fn bearer_token(&self) -> Option<&str> {
        match self {
            Self::BearerToken(token)
            | Self::ApiKeyAndBearer {
                bearer_token: token,
                ..
            } => Some(token),
            Self::None | Self::ApiKey(_) => None,
        }
    }

    #[must_use]
    pub fn masked_authorization_header(&self) -> &'static str {
        if self.bearer_token().is_some() {
            "Bearer [REDACTED]"
        } else {
            "<absent>"
        }
    }

    pub fn apply(&self, mut request_builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(api_key) = self.api_key() {
            request_builder = request_builder.header("x-api-key", api_key);
        }
        if let Some(token) = self.bearer_token() {
            request_builder = request_builder.bearer_auth(token);
        }
        request_builder
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OAuthTokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl From<OAuthTokenSet> for AuthSource {
    fn from(value: OAuthTokenSet) -> Self {
        Self::BearerToken(value.access_token)
    }
}

/// Latched once a surface has 400'd rejecting the `context-management` beta, so
/// context editing forces off process-wide from then on (see
/// [`mark_context_edit_surface_unsupported`]). Process-global rather than
/// per-client: the same surface backs the foreground client and every sub-agent
/// client, so one probe's finding must silence them all.
static CONTEXT_EDIT_SURFACE_UNSUPPORTED: AtomicBool = AtomicBool::new(false);

// Default off: a firing invalidates and re-bills the cached prefix at the 1h write premium,
// a measured net loss at realistic sizes; the local proportional microcompact tier
// (clearable >= context/5) is the default hygiene path.
const CONTEXT_EDIT_CLEAR_AT_LEAST_TOKENS: u64 = 20_000;

/// Server-side context editing (`clear_tool_uses`) on Anthropic requests:
/// **default OFF**, opt in with `ZO_ANTHROPIC_CONTEXT_EDIT=1|true|on|yes`.
/// A firing invalidates and re-bills the cached prefix at the 1h write premium,
/// a measured net loss at realistic context sizes; the local proportional
/// microcompact tier (clearable >= context/5) is the default hygiene path and
/// stands down while this is active (see `anthropic_server_trim_active`).
/// Forced off once a surface proved it lacks the beta — a 400 latched
/// `CONTEXT_EDIT_SURFACE_UNSUPPORTED` — which hands
/// tool-result hygiene back to the local tier without a restart.
#[must_use]
pub fn anthropic_context_editing_enabled() -> bool {
    if CONTEXT_EDIT_SURFACE_UNSUPPORTED.load(Ordering::Relaxed) {
        return false;
    }
    match std::env::var("ZO_ANTHROPIC_CONTEXT_EDIT") {
        Ok(raw) => matches!(raw.trim(), "1" | "true" | "on" | "yes"),
        Err(_) => false,
    }
}

/// Latch context editing off for the rest of the process after a surface
/// returned a 400 rejecting the `context-management` beta. Idempotent; logs
/// once on the transition so the fallback to local trimming is diagnosable.
pub fn mark_context_edit_surface_unsupported() {
    if !CONTEXT_EDIT_SURFACE_UNSUPPORTED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "[zo] Anthropic surface rejected the context-management beta; \
             using local context trimming for the rest of this process"
        );
    }
}

/// Whether this failed attempt is resent at once, because the error taught the
/// client something the retry will carry: the Claude Code version the API
/// wants, or that the `context-management` edit this attempt sent is not
/// served — in which case the process-global latch is lowered here, even if
/// another in-flight 400 already lowered it before this response was handled.
fn retries_at_once(error: &ApiError, context_edit_sent: bool) -> bool {
    if learned_newer_client_version(error) {
        return true;
    }
    if context_edit_sent && is_context_edit_unsupported(error) {
        mark_context_edit_surface_unsupported();
        return true;
    }
    false
}

/// True when a 400 says the presented Claude Code version is too old for the
/// model — and the version it wants was just learned, so the retry presents
/// it. Learned once per named version: a repeated rejection at the same
/// number answers `false` and falls through to the ordinary error.
fn learned_newer_client_version(error: &ApiError) -> bool {
    let ApiError::Api {
        status,
        message,
        body,
        ..
    } = error
    else {
        return false;
    };
    if status.as_u16() != 400 {
        return false;
    }
    message
        .as_deref()
        .and_then(telemetry::required_claude_code_version_in)
        .or_else(|| telemetry::required_claude_code_version_in(body))
        .is_some_and(|required| telemetry::learn_claude_code_version(&required))
}

/// True when `error` is a 400 rejecting the `context-management` beta / its
/// `clear_tool_uses` edit — the signal to drop context editing and retry. Kept
/// broad (checks both the parsed message and the raw body) so a wording change
/// on the API side doesn't silently defeat the fallback.
fn is_context_edit_unsupported(error: &ApiError) -> bool {
    let ApiError::Api {
        status,
        message,
        body,
        ..
    } = error
    else {
        return false;
    };
    if status.as_u16() != 400 {
        return false;
    }
    let mentions_context_edit = |text: &str| {
        text.contains("context-management")
            || text.contains("context_management")
            || text.contains("clear_tool_uses")
    };
    message.as_deref().is_some_and(mentions_context_edit) || mentions_context_edit(body)
}

/// The `anthropic-beta` header one attempt sends: the profile's betas, minus
/// context editing once a surface refused it, plus the preserved-thinking
/// control when the request binds its thinking (`bind_thinking_for_wire`).
fn wire_beta_header(profile_betas: &str, context_edit_on: bool, bind_thinking: bool) -> String {
    let value = if context_edit_on {
        profile_betas.to_string()
    } else {
        strip_context_management_beta(profile_betas)
    };
    if bind_thinking {
        with_beta(&value, THINKING_BINDING_BETA)
    } else {
        value
    }
}

/// `value` with `beta` joined on — once.
fn with_beta(value: &str, beta: &str) -> String {
    if value.split(',').any(|held| held.trim() == beta) {
        return value.to_string();
    }
    if value.is_empty() {
        return beta.to_string();
    }
    format!("{value},{beta}")
}

/// Drop the `context-management` beta from a comma-joined `anthropic-beta`
/// header value, preserving order and the other betas. Returns the value
/// unchanged when the beta is absent.
fn strip_context_management_beta(betas: &str) -> String {
    betas
        .split(',')
        .filter(|beta| !beta.trim().starts_with("context-management"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Reject a serialized body that already exceeds the endpoint's DECLARED
/// request-size ceiling, before it is uploaded.
///
/// Returns `None` — i.e. send it — whenever the catalog declares no ceiling for
/// this model. An undeclared limit means "unknown", never "unlimited", and
/// inventing a number would either reject valid requests or miss real ones; the
/// reactive 413 path still covers that case.
///
/// The synthesized error deliberately mirrors a real provider 413 (status,
/// `request_too_large` type) so it classifies as
/// [`crate::ProviderErrorClass::ContextOverflow`] through the ordinary path and
/// reaches the same compact-then-retry recovery. It adds the two numbers the
/// provider's own message never carries — the measured size and the ceiling —
/// because "how far over am I" is the only thing that tells a user whether
/// compaction can plausibly fix it or an attachment has to go.
fn request_too_large_preflight(model: &str, payload_len: usize) -> Option<ApiError> {
    let limit = crate::providers::max_request_bytes_for_model(model)?;
    let measured = payload_len as u64;
    if measured <= limit {
        return None;
    }
    Some(ApiError::Api {
        status: reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        error_type: Some("request_too_large".to_string()),
        message: Some(format!(
            "Request exceeds the maximum size: {measured} bytes serialized, endpoint accepts {limit}"
        )),
        body: String::new(),
        retryable: false,
        retry_after: None,
    })
}

fn context_edit_analytics_event(
    request_id: Option<&str>,
    context_management: Option<&ContextManagementResponse>,
) -> Option<AnalyticsEvent> {
    let context_management = context_management?;
    if context_management.applied_edits.is_empty() {
        return None;
    }
    Some(
        AnalyticsEvent::new("api", "context_edit_applied")
            .with_property(
                "request_id",
                request_id.map_or(Value::Null, |id| Value::String(id.to_string())),
            )
            .with_property(
                "applied_edit_count",
                Value::from(
                    u64::try_from(context_management.applied_edits.len()).unwrap_or(u64::MAX),
                ),
            )
            .with_property(
                "cleared_tool_uses",
                Value::from(context_management.cleared_tool_uses()),
            )
            .with_property(
                "cleared_input_tokens",
                Value::from(context_management.cleared_input_tokens()),
            ),
    )
}

/// Notification emitted just before the Anthropic client parks for a retry.
#[derive(Debug, Clone)]
/// One "retrying in Ns" row, raised whenever this client backs off — both at
/// establish time (the `send_with_retry` ladder) and mid-stream (a transparent
/// pre-commit re-open).
///
/// Carries the flattened `error` rather than a pre-chewed `rate_limited` flag:
/// the renderer labels it through `core_types::retry_signal::retry_notice_label`,
/// the same classifier the backoff schedule uses. The old boolean was a second,
/// coarser classifier that could not say "provider overloaded" at all, so an
/// overload rendered as "transient provider error" while the retry layer called
/// it capacity pressure.
pub struct AnthropicRetryNotice {
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay: Duration,
    pub error: String,
}

type RetryNoticeCallback = Arc<dyn Fn(AnthropicRetryNotice) + Send + Sync>;

#[derive(Clone)]
pub struct AnthropicClient {
    http: reqwest::Client,
    auth: AuthSource,
    base_url: String,
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
    fail_fast_on_rate_limit: bool,
    request_profile: AnthropicRequestProfile,
    session_tracer: Option<SessionTracer>,
    prompt_cache: Option<PromptCache>,
    last_prompt_cache_record: Arc<Mutex<Option<PromptCacheRecord>>>,
    retry_notice: Option<RetryNoticeCallback>,
}

impl fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("auth", &self.auth.redacted_debug())
            .field("base_url", &self.base_url)
            .field("max_retries", &self.max_retries)
            .field("initial_backoff", &self.initial_backoff)
            .field("max_backoff", &self.max_backoff)
            .field("fail_fast_on_rate_limit", &self.fail_fast_on_rate_limit)
            .field("request_profile", &self.request_profile)
            .field("has_session_tracer", &self.session_tracer.is_some())
            .field("has_prompt_cache", &self.prompt_cache.is_some())
            .field("has_retry_notice", &self.retry_notice.is_some())
            .finish_non_exhaustive()
    }
}

impl AuthSource {
    fn redacted_debug(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::ApiKey(_) => "ApiKey(<redacted>)",
            Self::BearerToken(_) => "BearerToken(<redacted>)",
            Self::ApiKeyAndBearer { .. } => "ApiKeyAndBearer(<redacted>)",
        }
    }
}

impl AnthropicClient {
    /// Fire-and-forget HEAD request to prime the connection pool so the
    /// first real `messages` call reuses an established TLS session.
    /// Errors are intentionally swallowed — this is best-effort warmup.
    pub async fn warm_connection(&self) {
        if matches!(self.auth, AuthSource::None) {
            return;
        }
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let _ = self.http.head(&url).send().await;
    }

    /// Ask the provider how many input tokens `request` actually costs.
    ///
    /// `POST /v1/messages/count_tokens` — the only source of truth for the
    /// number every budget in this repo is expressed in. Everything else
    /// (`estimate_system_prompt_tokens`, the harness report, the compaction
    /// ladder) uses a `chars / 4` approximation, which is fine for a guard that
    /// only has to be roughly right and NOT fine for a release gate that claims
    /// "fewer tokens than CC and codex". This is what turns that claim into
    /// something checkable.
    ///
    /// Counting is not billed and returns no completion, but it IS a network
    /// call: callers that must stay offline should not reach for it.
    ///
    /// # Errors
    /// Transport failures, auth failures, and any non-2xx response.
    pub async fn count_input_tokens(&self, request: &MessageRequest) -> Result<u32, ApiError> {
        let url = format!(
            "{}/v1/messages/count_tokens",
            self.base_url.trim_end_matches('/')
        );
        // The endpoint takes the request's prompt-side fields only; `max_tokens`
        // and streaming describe the completion, which is not being counted.
        // Absent, not null: the endpoint rejects `"tools": null`, and sending
        // it is how the tools-excluded half of the calibration silently failed.
        let mut payload = serde_json::Map::new();
        payload.insert(
            "model".to_string(),
            serde_json::Value::from(crate::wire_model_id(&request.model)),
        );
        payload.insert(
            "messages".to_string(),
            serde_json::to_value(&request.messages).unwrap_or_default(),
        );
        if let Some(system) = &request.system {
            payload.insert(
                "system".to_string(),
                serde_json::to_value(system).unwrap_or_default(),
            );
        }
        if let Some(tools) = &request.tools {
            payload.insert(
                "tools".to_string(),
                serde_json::to_value(tools).unwrap_or_default(),
            );
        }
        let payload = serde_json::Value::Object(payload);
        let mut builder = self
            .http
            .post(&url)
            .header("content-type", "application/json");
        builder = self.auth.apply(builder);
        for (header_name, header_value) in self.request_profile.header_pairs() {
            if header_value.is_empty() {
                continue;
            }
            builder = builder.header(header_name, header_value);
        }
        let response = builder
            .body(serde_json::to_vec(&payload).unwrap_or_default())
            .send()
            .await
            .map_err(ApiError::Http)?;
        let status = response.status();
        let body = response.text().await.map_err(ApiError::Http)?;
        if !status.is_success() {
            return Err(ApiError::Api {
                status,
                error_type: None,
                message: Some("count_tokens".to_string()),
                body: body.chars().take(300).collect(),
                retryable: false,
                retry_after: None,
            });
        }
        serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| value.get("input_tokens")?.as_u64())
            .and_then(|tokens| u32::try_from(tokens).ok())
            .ok_or_else(|| ApiError::Auth(format!(
                "count_tokens returned no input_tokens: {}",
                body.chars().take(200).collect::<String>()
            )))
    }

    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            http: super::shared_http_client(),
            auth: AuthSource::ApiKey(api_key.into()),
            base_url: DEFAULT_BASE_URL.to_string(),
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            fail_fast_on_rate_limit: false,
            request_profile: AnthropicRequestProfile::default(),
            session_tracer: None,
            prompt_cache: None,
            last_prompt_cache_record: Arc::new(Mutex::new(None)),
            retry_notice: None,
        }
    }

    #[must_use]
    pub fn from_auth(auth: AuthSource) -> Self {
        Self {
            http: super::shared_http_client(),
            auth,
            base_url: DEFAULT_BASE_URL.to_string(),
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            fail_fast_on_rate_limit: false,
            request_profile: AnthropicRequestProfile::default(),
            session_tracer: None,
            prompt_cache: None,
            last_prompt_cache_record: Arc::new(Mutex::new(None)),
            retry_notice: None,
        }
    }

    pub fn from_env() -> Result<Self, ApiError> {
        Ok(Self::from_auth(AuthSource::from_env_or_saved()?).with_base_url(read_base_url()))
    }

    #[must_use]
    pub fn with_auth_source(mut self, auth: AuthSource) -> Self {
        self.auth = auth;
        self
    }

    #[must_use]
    pub fn with_auth_token(mut self, auth_token: Option<String>) -> Self {
        match (
            self.auth.api_key().map(ToOwned::to_owned),
            auth_token.filter(|token| !token.is_empty()),
        ) {
            (Some(api_key), Some(bearer_token)) => {
                self.auth = AuthSource::ApiKeyAndBearer {
                    api_key,
                    bearer_token,
                };
            }
            (Some(api_key), None) => {
                self.auth = AuthSource::ApiKey(api_key);
            }
            (None, Some(bearer_token)) => {
                self.auth = AuthSource::BearerToken(bearer_token);
            }
            (None, None) => {
                self.auth = AuthSource::None;
            }
        }
        self
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    #[must_use]
    pub fn with_auth(mut self, auth: AuthSource) -> Self {
        self.auth = auth;
        self
    }

    /// Replace the auth source in place. Mirrors [`Self::with_auth_source`] but
    /// mutates, so a long-lived client can pick up a refreshed OAuth bearer
    /// mid-session without rebuilding (which would drop its prompt cache and
    /// HTTP pool). The base URL, beta headers, and cache are untouched.
    pub fn set_auth(&mut self, auth: AuthSource) {
        self.auth = auth;
    }

    /// The credential this client currently sends. Lets a 401-recovery path
    /// compare its bearer against the process-wide cache to detect that
    /// another thread already refreshed past it.
    #[must_use]
    pub fn auth(&self) -> &AuthSource {
        &self.auth
    }

    /// Swap the underlying HTTP client. Lets one-shot flows (keychain OAuth
    /// refresh) run with explicit connect/total timeouts instead of the shared
    /// pool's defaults, so a blackholed network bounds the call instead of
    /// hanging whatever thread is resolving credentials.
    #[must_use]
    pub(crate) fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    #[must_use]
    pub fn with_retry_policy(
        mut self,
        max_retries: u32,
        initial_backoff: Duration,
        max_backoff: Duration,
    ) -> Self {
        self.max_retries = max_retries;
        self.initial_backoff = initial_backoff;
        self.max_backoff = max_backoff;
        self
    }

    /// Return a 429/529 to the caller after the first response instead of
    /// consuming this client's same-provider retry ladder. Background agents
    /// enable this only when the Smart Router has already supplied a usable
    /// alternate model; foreground and no-fallback clients keep the default.
    #[must_use]
    pub fn with_rate_limit_fail_fast(mut self) -> Self {
        self.fail_fast_on_rate_limit = true;
        self
    }

    #[must_use]
    pub fn with_session_tracer(mut self, session_tracer: SessionTracer) -> Self {
        self.session_tracer = Some(session_tracer);
        self
    }

    #[must_use]
    pub fn with_retry_notice_callback(
        mut self,
        callback: impl Fn(AnthropicRetryNotice) + Send + Sync + 'static,
    ) -> Self {
        self.retry_notice = Some(Arc::new(callback));
        self
    }

    #[must_use]
    pub fn with_client_identity(mut self, client_identity: ClientIdentity) -> Self {
        self.request_profile.client_identity = client_identity;
        self
    }

    #[must_use]
    pub fn with_beta(mut self, beta: impl Into<String>) -> Self {
        self.request_profile = self.request_profile.with_beta(beta);
        self
    }

    #[must_use]
    pub fn with_extra_body_param(mut self, key: impl Into<String>, value: Value) -> Self {
        self.request_profile = self.request_profile.with_extra_body(key, value);
        self
    }

    /// Server-side trim (**default OFF**, opt in with
    /// `ZO_ANTHROPIC_CONTEXT_EDIT=1|true|on|yes`): when
    /// [`anthropic_context_editing_enabled`], attach the context-management
    /// beta and a `clear_tool_uses` edit so the API clears old tool results
    /// server-side. A firing invalidates and re-bills the cached prefix at the
    /// 1h write premium, a measured net loss at realistic context sizes; the
    /// local proportional microcompact tier (clearable >= context/5) is the
    /// default hygiene path. This attaches optimistically: a surface that lacks
    /// the beta 400s, which `send_with_retry` catches to latch the beta off
    /// process-wide and retry without it (handing hygiene back to the local
    /// microcompact tier). The per-request strip in `send_raw_request` then
    /// keeps it off for every later request.
    #[must_use]
    pub fn with_env_context_editing(self) -> Self {
        if !anthropic_context_editing_enabled() {
            return self;
        }
        self.with_beta("context-management-2025-06-27")
            .with_extra_body_param(
                "context_management",
                serde_json::json!({
                    "edits": [{
                        "type": "clear_tool_uses_20250919",
                        "clear_at_least": {
                            "type": "input_tokens",
                            "value": CONTEXT_EDIT_CLEAR_AT_LEAST_TOKENS
                        }
                    }]
                }),
            )
    }

    #[must_use]
    pub fn with_prompt_cache(mut self, prompt_cache: PromptCache) -> Self {
        self.prompt_cache = Some(prompt_cache);
        self
    }

    #[must_use]
    pub fn prompt_cache_stats(&self) -> Option<PromptCacheStats> {
        self.prompt_cache.as_ref().map(PromptCache::stats)
    }

    #[must_use]
    pub fn request_profile(&self) -> &AnthropicRequestProfile {
        &self.request_profile
    }

    #[must_use]
    pub fn session_tracer(&self) -> Option<&SessionTracer> {
        self.session_tracer.as_ref()
    }

    #[must_use]
    pub fn prompt_cache(&self) -> Option<&PromptCache> {
        self.prompt_cache.as_ref()
    }

    #[must_use]
    pub fn take_last_prompt_cache_record(&self) -> Option<PromptCacheRecord> {
        self.last_prompt_cache_record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    #[must_use]
    pub fn auth_source(&self) -> &AuthSource {
        &self.auth
    }

    /// Returns a reference to the underlying HTTP client.
    #[must_use]
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http
    }

    /// Returns the configured base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn missing_auth_error() -> ApiError {
        ApiError::Auth(
            "Claude auth unavailable; run `/login claude` before sending Anthropic requests."
                .to_string(),
        )
    }

    fn allow_unauthenticated_request_to_base_url(&self) -> bool {
        reqwest::Url::parse(&self.base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .is_some_and(|host| {
                host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|addr| addr.is_loopback())
            })
    }

    fn ensure_authenticated_for_request(&self) -> Result<(), ApiError> {
        if matches!(self.auth, AuthSource::None)
            && !self.allow_unauthenticated_request_to_base_url()
        {
            return Err(Self::missing_auth_error());
        }
        Ok(())
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        self.ensure_authenticated_for_request()?;
        let request = MessageRequest {
            stream: false,
            ..request.clone()
        };

        if let Some(prompt_cache) = &self.prompt_cache {
            if let Some(response) = prompt_cache.lookup_completion(&request) {
                return Ok(response);
            }
        }

        let response = self.send_with_retry(&request).await?;
        let request_id = request_id_from_headers(response.headers());
        let mut response = response
            .json::<MessageResponse>()
            .await
            .map_err(ApiError::from)?;
        if response.request_id.is_none() {
            response.request_id = request_id;
        }

        if let Some(prompt_cache) = &self.prompt_cache {
            let record = prompt_cache.record_response(&request, &response);
            self.store_last_prompt_cache_record(record);
        }
        if let Some(session_tracer) = &self.session_tracer {
            if let Some(event) = context_edit_analytics_event(
                response.request_id.as_deref(),
                response.context_management.as_ref(),
            ) {
                session_tracer.record_analytics(event);
            }
            session_tracer.record_analytics(
                AnalyticsEvent::new("api", "message_usage")
                    .with_property(
                        "request_id",
                        response
                            .request_id
                            .clone()
                            .map_or(Value::Null, Value::String),
                    )
                    .with_property("total_tokens", Value::from(response.total_tokens()))
                    .with_property(
                        "estimated_cost_usd",
                        Value::String(format_telemetry_usd(
                            response
                                .usage
                                .estimated_cost_usd(&response.model)
                                .total_cost_usd(),
                        )),
                    ),
            );
        }
        Ok(response)
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        self.ensure_authenticated_for_request()?;
        let response = self
            .send_with_retry(&request.clone().with_streaming())
            .await?;
        let rate_limit = ratelimit_from_headers(response.headers());
        // OAuth-first: publish the unified-window reading process-wide so the
        // sub-agent headroom gate / starvation labels see live quota.
        if let Some(snapshot) = rate_limit {
            crate::quota::record_rate_limit_snapshot(snapshot);
        }
        Ok(MessageStream {
            request_id: request_id_from_headers(response.headers()),
            rate_limit,
            response,
            parser: SseParser::new(),
            pending: VecDeque::new(),
            scratch: Vec::new(),
            done: false,
            request: request.clone(),
            prompt_cache: self.prompt_cache.clone(),
            latest_usage: None,
            usage_recorded: false,
            last_prompt_cache_record: Arc::clone(&self.last_prompt_cache_record),
            client: self.clone(),
            committed: false,
            restart_attempts: 0,
            restart_started_at: None,
        })
    }

    pub async fn exchange_oauth_code(
        &self,
        config: &OAuthConfig,
        request: &OAuthTokenExchangeRequest,
    ) -> Result<OAuthTokenSet, ApiError> {
        let response = self
            .http
            .post(&config.token_url)
            .header("content-type", "application/x-www-form-urlencoded")
            .form(&request.form_params())
            .send()
            .await
            .map_err(ApiError::from)?;
        let response = expect_success(response).await?;
        let raw = response
            .json::<OAuthTokenResponse>()
            .await
            .map_err(ApiError::from)?;
        Ok(raw.into_token_set(now_unix_timestamp()))
    }

    pub async fn refresh_oauth_token(
        &self,
        config: &OAuthConfig,
        request: &OAuthRefreshRequest,
    ) -> Result<OAuthTokenSet, ApiError> {
        let response = self
            .http
            .post(&config.token_url)
            .header("content-type", "application/x-www-form-urlencoded")
            .form(&request.form_params())
            .send()
            .await
            .map_err(ApiError::from)?;
        let response = expect_success(response).await?;
        let raw = response
            .json::<OAuthTokenResponse>()
            .await
            .map_err(ApiError::from)?;
        Ok(raw.into_token_set(now_unix_timestamp()))
    }

    /// The most this ladder would ever wait in total — every retry at its
    /// cap — which is the longest server hint it makes sense to obey here. A
    /// hint beyond it belongs to the layer that can change model, not to
    /// this one, which can only sleep.
    fn retry_reach(&self) -> Duration {
        self.max_backoff.max(DEFAULT_MAX_BACKOFF) * (self.max_retries + 1)
    }

    /// Whether `error` names a wall the server says lifts beyond this
    /// ladder's reach. Taking the ladder's cap against such a wall neither
    /// obeys the hint nor escapes it: on 2026-09-21 thirty-one requests each
    /// slept 30 s up to six times against a `retry-after` of 154,912 s. Past
    /// the reach the hint is an instruction to stop, and the error goes up
    /// with the hint still on it for the runtime's escape — a fallback model,
    /// or a plain notice — to read at once.
    fn wall_beyond_reach(&self, error: Option<&ApiError>) -> bool {
        error
            .and_then(ApiError::retry_after)
            .is_some_and(|hint| hint > self.retry_reach())
    }

    /// The refusal owed to a wall this process inherited rather than earned,
    /// or `None` when nothing is holding requests back.
    ///
    /// [`crate::quota::rate_limit_cooldown_remaining_ms`] is the one place
    /// that answers "hold requests to this provider?" — this process's own
    /// 429s unioned with what other `zo` processes wrote to the shared
    /// registry — and the verify bridge already asks it by the same function
    /// before it picks a cross-model verifier. Asking it here, *before* the
    /// request, is what stops a fresh process re-earning a wall the last one
    /// already paid for: on 2026-09-21 an account 429 named a `retry-after`
    /// of 154,912 s and each new `zo` still opened by sending, because a
    /// cool-down was only ever consulted after a response came back. Refused
    /// in front of the wire, the runtime's escape — a fallback model, or a
    /// plain notice — runs on the first ask.
    ///
    /// [`crate::quota::took_own_rate_limit`] is what keeps this a door and
    /// not a gag. Once this process has taken its own 429 the park is its own
    /// ladder's arithmetic, and hard-refusing on that overrides the layers
    /// that own the timing: measured, a provider answering `retry-after: 0`
    /// became minutes of self-imposed silence and a `/goal` that should have
    /// resumed on the next attempt never did.
    ///
    /// Independent of the retry policy either way. `fail_fast_on_rate_limit`
    /// and the ladder decide what to do with an answer that *arrived*; this
    /// never sends, never sleeps and never spends an attempt.
    fn parked_window_refusal() -> Option<ApiError> {
        let kind = super::ProviderKind::Anthropic;
        if crate::quota::took_own_rate_limit(kind) {
            return None;
        }
        let parked = Duration::from_millis(crate::quota::rate_limit_cooldown_remaining_ms(kind));
        (!parked.is_zero()).then(|| ApiError::rate_limited_before_send("anthropic", parked))
    }

    /// Sleep one rung of the retry ladder, announcing it first.
    ///
    /// Split out of `send_with_retry` only because the ladder outgrew the
    /// hundred-line lint; it is still one step of that loop and nothing else
    /// calls it.
    async fn wait_out_one_rung(
        &self,
        attempts: u32,
        last_error: Option<&ApiError>,
    ) -> Result<(), ApiError> {
        // Server-provided `Retry-After` is authoritative and used verbatim;
        // our own exponential backoff is jittered so N parallel agents
        // retrying the same 429 don't re-collide on an identical wakeup.
        let delay = match last_error.and_then(ApiError::retry_after) {
            Some(server) => server,
            None => super::retry_backoff::spread_backoff(self.backoff_for_attempt(attempts)?),
        };
        let capped = delay.min(self.max_backoff.max(Duration::from_secs(30)));
        if let (Some(callback), Some(error)) = (&self.retry_notice, last_error) {
            callback(AnthropicRetryNotice {
                attempt: attempts,
                max_attempts: self.max_retries + 1,
                delay: capped,
                error: error.to_string(),
            });
        }
        // Include the failure reason: without it the log shows bare retry
        // ladders and the underlying 429/529/network cause is undiagnosable
        // (the OTLP tracer that records it is env-gated and normally off).
        let reason = last_error.map_or_else(|| "unknown error".to_owned(), single_line_reason);
        eprintln!(
            "[zo] retrying in {:.1}s (attempt {}/{}): {reason}",
            capped.as_secs_f64(),
            attempts,
            self.max_retries + 1,
        );
        tokio::time::sleep(capped).await;
        Ok(())
    }

    async fn send_with_retry(
        &self,
        request: &MessageRequest,
    ) -> Result<reqwest::Response, ApiError> {
        if let Some(parked) = Self::parked_window_refusal() {
            return Err(parked);
        }
        let mut attempts = 0;
        let mut last_error: Option<ApiError>;
        let mut transport_streak: u32 = 0;
        let mut escape_http: Option<reqwest::Client> = None;

        loop {
            attempts += 1;
            if let Some(session_tracer) = &self.session_tracer {
                session_tracer.record_http_request_started(
                    attempts,
                    "POST",
                    "/v1/messages",
                    Map::new(),
                );
            }
            match self.send_raw_request(request, escape_http.as_ref()).await {
                Ok((response, context_edit_sent)) => match expect_success(response).await {
                    Ok(response) => {
                        if let Some(session_tracer) = &self.session_tracer {
                            session_tracer.record_http_request_succeeded(
                                attempts,
                                "POST",
                                "/v1/messages",
                                response.status().as_u16(),
                                request_id_from_headers(response.headers()),
                                Map::new(),
                            );
                        }
                        return Ok(response);
                    }
                    Err(error) if retries_at_once(&error, context_edit_sent) => {
                        // A 400 that taught the client something — the version
                        // the API wants, or that context editing is not served
                        // here — is answered by the same request again, at
                        // once, without spending retry budget.
                        self.record_request_failure(attempts, &error);
                        attempts -= 1;
                        continue;
                    }
                    Err(error)
                        if error.is_retryable()
                            && !(self.fail_fast_on_rate_limit && error.is_rate_limit())
                            && attempts <= self.max_retries + 1 =>
                    {
                        self.record_request_failure(attempts, &error);
                        last_error = Some(error);
                    }
                    Err(error) => {
                        self.record_request_failure(attempts, &error);
                        return Err(error);
                    }
                },
                Err(error)
                    if error.is_retryable()
                        && !(self.fail_fast_on_rate_limit && error.is_rate_limit())
                        && attempts <= self.max_retries + 1 =>
                {
                    self.record_request_failure(attempts, &error);
                    last_error = Some(error);
                }
                Err(error) => {
                    self.record_request_failure(attempts, &error);
                    return Err(error);
                }
            }

            if attempts > self.max_retries || self.wall_beyond_reach(last_error.as_ref()) {
                break;
            }

            Self::escape_pool_on_transport_streak(
                last_error.as_ref(),
                &mut transport_streak,
                &mut escape_http,
            );

            self.wait_out_one_rung(attempts, last_error.as_ref()).await?;
        }

        Err(ApiError::RetriesExhausted {
            attempts,
            last_error: Box::new(last_error.expect("retry loop must capture an error")),
        })
    }

    /// Track consecutive transport-level send failures (connect/TLS/socket —
    /// no HTTP response at all) and, on the second in a row, arm a fresh
    /// connection pool for the request's remaining attempts. Two identical
    /// send failures point at the SHARED pool reusing a connection the
    /// keep-alive probe has not declared dead yet — re-probing the same
    /// corpse fails the whole ladder (measured live: zo-bench, 6/6 attempts
    /// dead while claude-code succeeded alongside). An HTTP-status error
    /// resets the streak: a response proves the transport works.
    fn escape_pool_on_transport_streak(
        last_error: Option<&ApiError>,
        transport_streak: &mut u32,
        escape_http: &mut Option<reqwest::Client>,
    ) {
        if matches!(last_error, Some(ApiError::Http(_))) {
            *transport_streak += 1;
        } else {
            *transport_streak = 0;
        }
        if *transport_streak >= 2 && escape_http.is_none() {
            *escape_http = Some(super::poison_escape_http_client());
            eprintln!(
                "[zo] transport failures persist on the pooled connection — retrying on a fresh connection pool"
            );
        }
    }

    async fn send_raw_request(
        &self,
        request: &MessageRequest,
        http_override: Option<&reqwest::Client>,
    ) -> Result<(reqwest::Response, bool), ApiError> {
        // Cloud-gateway routing (Bedrock/Vertex): every Anthropic request —
        // foreground, sub-agent, mid-stream restart — flows through this one
        // chokepoint, so the URL/auth/body rewrite here covers them all. A
        // misconfigured gateway fails loudly instead of silently falling back
        // to the first-party API.
        let gateway = match super::cloud_gateway::active() {
            Some(Ok(gateway)) => Some(gateway),
            Some(Err(message)) => return Err(ApiError::Auth(message.clone())),
            None => None,
        };
        let request_url = gateway.map_or_else(
            || format!("{}/v1/messages", self.base_url.trim_end_matches('/')),
            |gateway| gateway.request_url(&request.model, request.stream),
        );
        let request_builder = http_override
            .unwrap_or(&self.http)
            .post(&request_url)
            .header("content-type", "application/json");

        // Normalize the thinking/effort shape for this model before rendering:
        // adaptive-thinking models (Opus 4.6+/Fable) must receive
        // `output_config.effort` + `thinking:{type:"adaptive"}`, never the
        // deprecated `thinking.budget_tokens`. This one chokepoint covers every
        // path (foreground, stream, sub-agent, gateway). `Cow` avoids cloning
        // the (large) request when no rewrite is needed — the legacy case.
        let normalized = normalize_thinking_for_wire(request);
        // With the final thinking state now known, drop any replayed reasoning
        // blocks if this request has thinking disabled — a thinking block with no
        // thinking config on the request 400s and wedges the session.
        let normalized = strip_thinking_blocks_when_disabled(normalized);
        let normalized = normalize_tool_choice_for_wire(normalized);
        // Preserved thinking (Fable/Mythos 5.1): the field and its beta header
        // travel together — see `bind_thinking_for_wire` and `wire_beta_header`.
        let bind_thinking =
            gateway.is_none() && super::preserved_thinking_generation(&request.model);
        let normalized = bind_thinking_for_wire(normalized, bind_thinking);

        // The body must be finalized (gateway rewrites applied) *before* auth,
        // because SigV4 signs over the exact bytes. Serialize once and send the
        // same bytes so the signature matches the wire payload.
        let mut request_body = self.request_profile.render_json_body(normalized.as_ref())?;
        // Apply the catalog's served id, if it declares one. Placed here rather
        // than on the request struct so it lands before both the gateway rewrite
        // and serialization — SigV4 signs these exact bytes, so the model must
        // be final by now. Normally a no-op: an Anthropic model's selection id
        // is its served id, and the catalog answers `None`.
        if let Some(object) = request_body.as_object_mut() {
            let wire = crate::providers::wire_model_for_request(
                &request.model,
                request.reasoning_request(),
            );
            if wire != request.model {
                object.insert("model".to_string(), Value::String(wire));
            }
        }
        // Capture what this exact attempt is about to send. The process-global
        // latch can change while the request is in flight, so the 400 fallback
        // must not re-read global state when the response arrives.
        let context_edit_on = anthropic_context_editing_enabled();
        let context_edit_configured = request_body.get("context_management").is_some()
            || self
                .request_profile
                .betas
                .iter()
                .any(|beta| beta.starts_with("context-management"));
        let context_edit_sent = context_edit_on && context_edit_configured;
        if !context_edit_on {
            if let Some(object) = request_body.as_object_mut() {
                object.remove("context_management");
            }
        }
        if let Some(gateway) = gateway {
            gateway.adapt_body(&mut request_body);
        }
        let payload = serde_json::to_vec(&request_body).map_err(ApiError::Json)?;
        // Preflight the body against the endpoint's declared byte ceiling. This
        // is the only point where the EXACT wire size is known, and uploading a
        // body the endpoint will reject costs the whole transfer before the 413
        // comes back — on a multi-megabyte attachment-heavy turn that is the
        // slowest possible way to learn nothing. The synthesized error is
        // byte-identical in class to a real 413, so it lands in the same
        // `ContextOverflow` recovery (compact, then re-send) one layer up.
        if let Some(error) = request_too_large_preflight(&request.model, payload.len()) {
            return Err(error);
        }

        let mut request_builder = match gateway {
            // The gateway's credential replaces the first-party auth chain.
            Some(gateway) => {
                gateway
                    .apply_auth(request_builder, &request_url, &payload)
                    .await?
            }
            None => self.auth.apply(request_builder),
        };
        let mut beta_header_sent = false;
        for (header_name, header_value) in self.request_profile.header_pairs() {
            let header_value = if header_name == "anthropic-beta" {
                beta_header_sent = true;
                wire_beta_header(&header_value, context_edit_on, bind_thinking)
            } else {
                header_value
            };
            if header_value.is_empty() {
                continue;
            }
            request_builder = request_builder.header(header_name, header_value);
        }
        if bind_thinking && !beta_header_sent {
            request_builder = request_builder.header("anthropic-beta", THINKING_BINDING_BETA);
        }
        request_builder = request_builder.body(payload);

        // Debug: log request details on first attempt
        if std::env::var("ZO_DEBUG_AUTH").is_ok() {
            eprintln!("\x1b[36m[debug] POST {request_url}");
            eprintln!("[debug] auth: {}", self.auth.masked_authorization_header());
            eprintln!("[debug] has x-api-key: {}", self.auth.api_key().is_some());
            eprintln!("[debug] has bearer: {}", self.auth.bearer_token().is_some());
            for (k, v) in self.request_profile.header_pairs() {
                eprintln!("[debug] {k}: {v}");
            }
            eprintln!("[debug] model: {}", request.model);
            eprintln!(
                "[debug] body: {}\x1b[0m",
                serde_json::to_string(&request_body).unwrap_or_default()
            );
        }

        request_builder
            .send()
            .await
            .map(|response| (response, context_edit_sent))
            .map_err(ApiError::from)
    }

    fn record_request_failure(&self, attempt: u32, error: &ApiError) {
        if let Some(session_tracer) = &self.session_tracer {
            session_tracer.record_http_request_failed(
                attempt,
                "POST",
                "/v1/messages",
                error.to_string(),
                error.is_retryable(),
                Map::new(),
            );
        }
    }

    fn store_last_prompt_cache_record(&self, record: PromptCacheRecord) {
        *self
            .last_prompt_cache_record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(record);
    }

    fn backoff_for_attempt(&self, attempt: u32) -> Result<Duration, ApiError> {
        super::backoff_for_attempt(attempt, self.initial_backoff, self.max_backoff)
    }
}

static CACHED_AUTH: std::sync::OnceLock<std::sync::Mutex<AuthSource>> = std::sync::OnceLock::new();

impl AuthSource {
    /// Snapshot of the most recently cached auth resolution, if any.
    ///
    /// Subagents call this to inherit the parent runtime's credential
    /// without re-running `from_env_or_saved` — keeps OAuth refresh
    /// counts low and avoids racing the cache.
    #[must_use]
    pub fn cached() -> Option<Self> {
        CACHED_AUTH
            .get()
            .and_then(|lock| lock.lock().ok())
            .and_then(|guard| {
                if matches!(*guard, Self::None) {
                    None
                } else {
                    Some(guard.clone())
                }
            })
    }

    /// Cache a successfully resolved auth source so subagents inherit it.
    pub fn cache_resolved(auth: &Self) {
        if matches!(auth, Self::None) {
            return;
        }
        match CACHED_AUTH.get() {
            Some(lock) => {
                if let Ok(mut guard) = lock.lock() {
                    *guard = auth.clone();
                }
            }
            None => {
                let _ = CACHED_AUTH.set(std::sync::Mutex::new(auth.clone()));
            }
        }
    }

    pub fn from_env_or_saved() -> Result<Self, ApiError> {
        if let Some(api_key) = read_env_non_empty("ANTHROPIC_API_KEY")? {
            return match read_env_non_empty("ANTHROPIC_AUTH_TOKEN")? {
                Some(bearer_token) => Ok(Self::ApiKeyAndBearer {
                    api_key,
                    bearer_token,
                }),
                None => Ok(Self::ApiKey(api_key)),
            };
        }
        if let Some(bearer_token) = read_env_non_empty("ANTHROPIC_AUTH_TOKEN")? {
            return Ok(Self::BearerToken(bearer_token));
        }
        match load_saved_oauth_token() {
            Ok(Some(token_set)) if oauth_token_is_expired(&token_set) => {
                if token_set.refresh_token.is_some() {
                    Err(ApiError::Auth(
                        "saved OAuth token is expired; load runtime OAuth config to refresh it"
                            .to_string(),
                    ))
                } else {
                    Err(ApiError::ExpiredOAuthToken)
                }
            }
            Ok(Some(token_set)) => Ok(Self::BearerToken(token_set.access_token)),
            Ok(None) => Err(ApiError::missing_credentials(
                "Anthropic",
                &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
            )),
            Err(error) => Err(error),
        }
    }
}

/// Seconds of head-room before a token's hard expiry at which we already
/// treat it as expired and refresh. Without this, a token that lapses
/// between the local clock check and the server receiving the request
/// (clock skew / boundary race) slips through and the call fails with a
/// bare 401. Mirrors the 60s buffer already used for MCP OAuth tokens.
const OAUTH_EXPIRY_BUFFER_SECS: u64 = 60;

/// Raw OAuth token-endpoint response (RFC 6749 §5.1). The Anthropic endpoint
/// returns `scope` (space-separated string) and `expires_in` (seconds from now),
/// which do not map onto [`OAuthTokenSet`]'s `scopes: Vec` / `expires_at`
/// (absolute Unix seconds). Deserializing the response straight into
/// `OAuthTokenSet` silently dropped both — the saved token landed with
/// `scopes: []` and `expires_at: null`, so the `user:inference` scope check saw
/// nothing and refresh never fired. This shim translates the wire shape; an
/// absolute `expires_at` or array `scopes` are honored if the server sends them.
#[derive(serde::Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    scopes: Option<Vec<String>>,
}

impl OAuthTokenResponse {
    fn into_token_set(self, now: u64) -> OAuthTokenSet {
        let scopes = self.scopes.unwrap_or_else(|| {
            self.scope
                .as_deref()
                .map(|raw| raw.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default()
        });
        let expires_at = self
            .expires_at
            .or_else(|| self.expires_in.map(|secs| now.saturating_add(secs)));
        OAuthTokenSet {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at,
            scopes,
        }
    }
}

/// Render an [`ApiError`] as a single log-friendly line: newlines collapsed,
/// long bodies truncated so a retry notice stays one greppable row.
fn single_line_reason(error: &ApiError) -> String {
    const MAX_REASON_CHARS: usize = 220;
    let full = error.to_string();
    let mut chars = full
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch });
    let mut reason: String = chars.by_ref().take(MAX_REASON_CHARS).collect();
    if chars.next().is_some() {
        reason.push('…');
    }
    reason
}

#[must_use]
pub fn oauth_token_is_expired(token_set: &OAuthTokenSet) -> bool {
    token_set
        .expires_at
        .is_some_and(|expires_at| expires_at <= now_unix_timestamp() + OAUTH_EXPIRY_BUFFER_SECS)
}

pub fn resolve_saved_oauth_token(config: &OAuthConfig) -> Result<Option<OAuthTokenSet>, ApiError> {
    let Some(token_set) = load_saved_oauth_token()? else {
        return Ok(None);
    };
    resolve_saved_oauth_token_set(config, token_set).map(Some)
}

pub fn has_auth_from_env_or_saved() -> Result<bool, ApiError> {
    Ok(read_env_non_empty("ANTHROPIC_API_KEY")?.is_some()
        || read_env_non_empty("ANTHROPIC_AUTH_TOKEN")?.is_some()
        || load_saved_oauth_token()?.is_some())
}

pub fn resolve_startup_auth_source<F>(load_oauth_config: F) -> Result<AuthSource, ApiError>
where
    F: FnOnce() -> Result<Option<OAuthConfig>, ApiError>,
{
    if let Some(api_key) = read_env_non_empty("ANTHROPIC_API_KEY")? {
        return match read_env_non_empty("ANTHROPIC_AUTH_TOKEN")? {
            Some(bearer_token) => Ok(AuthSource::ApiKeyAndBearer {
                api_key,
                bearer_token,
            }),
            None => Ok(AuthSource::ApiKey(api_key)),
        };
    }
    if let Some(bearer_token) = read_env_non_empty("ANTHROPIC_AUTH_TOKEN")? {
        return Ok(AuthSource::BearerToken(bearer_token));
    }

    let Some(token_set) = load_saved_oauth_token()? else {
        return Err(ApiError::missing_credentials(
            "Anthropic",
            &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
        ));
    };
    if !oauth_token_is_expired(&token_set) {
        return Ok(AuthSource::BearerToken(token_set.access_token));
    }
    if token_set.refresh_token.is_none() {
        return Err(ApiError::ExpiredOAuthToken);
    }

    let Some(config) = load_oauth_config()? else {
        return Err(ApiError::Auth(
            "saved OAuth token is expired; runtime OAuth config is missing".to_string(),
        ));
    };
    // A refresh failure here surfaces as-is, and that is now worth something: the
    // refresh gate turns a rejected branch into "run `zo login claude`" instead of
    // a bare `invalid_grant` 400 body. The caller decides whether an
    // unauthenticated start is acceptable ([`StartupAuthPolicy`]); this only has
    // to say why, in words that name the fix.
    //
    // Deliberately *not* falling back to the keychain here: this function is only
    // reached once the shared chain — which tries the keychain first — has already
    // come up empty, so a keychain probe at this point could only repeat that
    // answer.
    Ok(AuthSource::from(resolve_saved_oauth_token_set(
        &config, token_set,
    )?))
}

/// Where a resolved Claude credential came from, in the subscription-first
/// priority order zo runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeAuthOrigin {
    /// `.credentials.json` under the IDE-provided `CLAUDE_CONFIG_DIR`.
    ManagedFile,
    /// Claude Code keychain session (subscription OAuth, shared with the CLI).
    Keychain,
    /// `zo login` OAuth saved in the credentials file.
    SavedOauth,
    /// `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` environment fallback.
    Env,
}

/// A resolved Claude credential plus the metadata callers schedule around.
#[derive(Debug, Clone)]
pub struct ResolvedClaudeAuth {
    pub auth: AuthSource,
    pub origin: ClaudeAuthOrigin,
    /// Hard expiry (Unix ms) when the origin records one, for proactive
    /// refresh scheduling.
    pub expires_at_ms: Option<u64>,
    /// File identity used to avoid reading an unchanged IDE-managed credential.
    pub managed_file_stamp: Option<keychain::ManagedCredentialsStamp>,
}

/// Full Claude credential resolution with refresh — the one chain every
/// consumer (interactive client, sub-agents, 401 recovery) shares. Zo is an
/// OAuth-subscription-first tool, so managed OAuth outranks static env keys:
/// 1) the IDE-managed Claude credentials file when `CLAUDE_CONFIG_DIR` names
///    one, otherwise the Claude Code keychain (refreshing an expired token in
///    place, exactly as Claude Code itself would),
/// 2) saved `zo login` OAuth (refreshing against the default subscription
///    config, so no `.zo` OAuth config is required),
/// 3) env `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` as the metered
///    fallback.
///
/// `None` when nothing usable exists. Safe to call from any context —
/// network refreshes hop to a dedicated thread.
#[must_use]
pub fn resolve_claude_auth_fresh_detailed() -> Option<ResolvedClaudeAuth> {
    resolve_claude_auth_fresh_explained().ok()
}

/// [`resolve_claude_auth_fresh_detailed`], and when nothing usable exists,
/// why: [`CredentialMiss::Absent`] only when no rung holds a Claude credential
/// at all, otherwise [`CredentialMiss::Unusable`] with the first rung's reason
/// in chain order — a managed or keychain login that expired and would not
/// refresh outranks a saved login that failed the same way.
pub fn resolve_claude_auth_fresh_explained() -> Result<ResolvedClaudeAuth, CredentialMiss> {
    let resolved = resolve_claude_auth_fresh_inner();
    if let Ok(ref auth) = resolved {
        // OAuth-first visibility: publish which rung of the chain answered so
        // the HUD can show a silent fall to the metered env key (the user is
        // on subscription by default and should notice paid-key usage).
        *crate::sync_bridge::lock_recovered(&LATEST_CLAUDE_AUTH_ORIGIN) = Some(auth.origin);
        crate::sync_bridge::lock_recovered(&LATEST_CLAUDE_MANAGED_STAMP)
            .clone_from(&auth.managed_file_stamp);
        // And the credential itself, for sub-agents to inherit — whoever
        // resolved it (a turn, a 401 recovery, a model-list refresh).
        AuthSource::cache_resolved(&auth.auth);
    }
    resolved
}

/// Most recent rung of the credential chain that satisfied a resolution this
/// process lifetime — `None` until the first resolve. Display-only signal.
#[must_use]
pub fn latest_claude_auth_origin() -> Option<ClaudeAuthOrigin> {
    *crate::sync_bridge::lock_recovered(&LATEST_CLAUDE_AUTH_ORIGIN)
}

/// Poison policy: recover — the only write is a `Copy` assignment.
static LATEST_CLAUDE_AUTH_ORIGIN: std::sync::Mutex<Option<ClaudeAuthOrigin>> =
    std::sync::Mutex::new(None);
static LATEST_CLAUDE_MANAGED_STAMP: std::sync::Mutex<
    Option<keychain::ManagedCredentialsStamp>,
> = std::sync::Mutex::new(None);

/// Whether the IDE-managed Claude credential file changed since resolution.
#[must_use]
pub fn managed_claude_auth_changed() -> bool {
    if latest_claude_auth_origin() != Some(ClaudeAuthOrigin::ManagedFile) {
        return false;
    }
    let previous = crate::sync_bridge::lock_recovered(&LATEST_CLAUDE_MANAGED_STAMP).clone();
    let current = keychain::managed_claude_credentials_stamp();
    previous.is_none() || current.is_none() || previous != current
}

fn resolve_claude_auth_fresh_inner() -> Result<ResolvedClaudeAuth, CredentialMiss> {
    let managed_file_stamp = keychain::managed_claude_credentials_stamp();
    let session_miss = match keychain::read_claude_code_keychain_session_explained() {
        Ok(session) => {
            return Ok(ResolvedClaudeAuth {
                auth: AuthSource::BearerToken(session.access_token),
                origin: if managed_file_stamp.is_some() {
                    ClaudeAuthOrigin::ManagedFile
                } else {
                    ClaudeAuthOrigin::Keychain
                },
                expires_at_ms: session.expires_at_ms,
                managed_file_stamp,
            });
        }
        Err(miss) => miss,
    };
    let config = keychain::claude_code_oauth_config();
    // Falling through on failure is deliberate — the env rung may still answer —
    // but it is no longer *silent*: `resolve_saved_oauth_token_set_with` reports
    // a dead branch once, with the re-authentication route. Before that, a user
    // whose saved login had been superseded saw only a downstream 401 and no hint
    // that `zo login claude` was the fix.
    let saved_miss = match resolve_saved_oauth_token_any_context(&config) {
        Ok(Some(token_set)) => {
            let expires_at_ms = token_set.expires_at.map(|secs| secs.saturating_mul(1000));
            return Ok(ResolvedClaudeAuth {
                auth: AuthSource::from(token_set),
                origin: ClaudeAuthOrigin::SavedOauth,
                expires_at_ms,
                managed_file_stamp: None,
            });
        }
        Ok(None) => CredentialMiss::Absent,
        Err(error) => CredentialMiss::Unusable(saved_login_unusable(&error)),
    };
    let env_miss = match AuthSource::from_env() {
        Ok(auth) => {
            return Ok(ResolvedClaudeAuth {
                auth,
                origin: ClaudeAuthOrigin::Env,
                expires_at_ms: None,
                managed_file_stamp: None,
            });
        }
        Err(ApiError::MissingCredentials { .. }) => CredentialMiss::Absent,
        Err(error) => CredentialMiss::Unusable(single_line_reason(&error)),
    };
    Err(session_miss.or(saved_miss).or(env_miss))
}

/// Whether this process holds a Claude credential on any rung of the chain —
/// configured, not necessarily usable. Offline and cheap: the environment,
/// zo's saved login (one file read; a store that cannot be read is there all
/// the same), the managed folder or the keychain item — no token is refreshed
/// and no secret is read. A model list asks it to tell a skip another process
/// wrote from this one's credential (t-6248).
#[must_use]
pub fn claude_credential_configured() -> bool {
    read_env_non_empty("ANTHROPIC_API_KEY").map_or(true, |key| key.is_some())
        || read_env_non_empty("ANTHROPIC_AUTH_TOKEN").map_or(true, |token| token.is_some())
        || load_oauth_credentials().map_or(true, |saved| saved.is_some())
        || keychain::claude_code_login_configured()
}

/// Why zo's own saved Claude login could not be used, for a person to read.
/// The refresh gate's refusals already name the way out; a token without a
/// refresh token, or a refresh the endpoint answered with an error, is said
/// here in the same terms.
fn saved_login_unusable(error: &ApiError) -> String {
    match error {
        ApiError::Auth(message) => message.clone(),
        ApiError::ExpiredOAuthToken => {
            "zo's saved Claude login expired and holds no refresh token — run `zo login claude`".to_string()
        }
        other => format!(
            "zo's saved Claude login could not be refreshed ({}) — run `zo login claude` if it keeps failing",
            single_line_reason(other)
        ),
    }
}

/// [`resolve_claude_auth_fresh_detailed`] reduced to the credential (cached
/// for sub-agent inheritance, as every fresh resolution is).
#[must_use]
pub fn resolve_claude_auth_fresh() -> Option<AuthSource> {
    resolve_claude_auth_fresh_detailed().map(|resolved| resolved.auth)
}

static REFRESH_FLIGHT: Mutex<()> = Mutex::new(());

/// One-shot recovery for a request that just 401'd: re-run the full resolution
/// chain (refreshing keychain / saved tokens as needed) under a process-wide
/// single-flight lock, so N parallel agents hitting the same lapse trigger one
/// refresh instead of N racing ones (a rotating refresh token tolerates being
/// spent once, not N times). `stale_bearer` is the credential that failed:
/// when another thread already refreshed past it the cached result is adopted
/// without touching the network, and when re-resolution yields the *same*
/// credential back (e.g. an env-pinned key) `None` reports that nothing
/// fresher exists.
#[must_use]
pub fn refresh_claude_auth_after_unauthorized(stale_bearer: Option<&str>) -> Option<AuthSource> {
    let _flight = REFRESH_FLIGHT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let (Some(cached), Some(stale)) = (AuthSource::cached(), stale_bearer) {
        if cached.bearer_token().is_some_and(|bearer| bearer != stale) {
            return Some(cached);
        }
    }
    // The keychain memo may still hold the bearer that just 401'd; force the
    // resolution below through to a real keychain read.
    keychain::invalidate_claude_code_keychain_cache();
    let fresh = resolve_claude_auth_fresh()?;
    if stale_bearer.is_some() && fresh.bearer_token() == stale_bearer {
        return None;
    }
    Some(fresh)
}

fn resolve_saved_oauth_token_set(
    config: &OAuthConfig,
    token_set: OAuthTokenSet,
) -> Result<OAuthTokenSet, ApiError> {
    resolve_saved_oauth_token_set_with(config, token_set, |config, request| {
        let client = AnthropicClient::from_auth(AuthSource::None).with_base_url(read_base_url());
        client_runtime_block_on(async { client.refresh_oauth_token(config, &request).await })
    })
}

/// [`resolve_saved_oauth_token`] variant whose refresh round-trip runs on a
/// dedicated thread, making it callable from inside an async context (the
/// sub-agent retry loop) where the `client_runtime_block_on` variant would
/// panic on the nested runtime.
fn resolve_saved_oauth_token_any_context(
    config: &OAuthConfig,
) -> Result<Option<OAuthTokenSet>, ApiError> {
    let Some(token_set) = load_saved_oauth_token()? else {
        return Ok(None);
    };
    resolve_saved_oauth_token_set_with(config, token_set, |config, request| {
        keychain::refresh_token_set_on_own_thread(config, &request)
    })
    .map(Some)
}

fn resolve_saved_oauth_token_set_with<F>(
    config: &OAuthConfig,
    mut token_set: OAuthTokenSet,
    refresh: F,
) -> Result<OAuthTokenSet, ApiError>
where
    F: FnOnce(&OAuthConfig, OAuthRefreshRequest) -> Result<OAuthTokenSet, ApiError>,
{
    if !oauth_token_is_expired(&token_set) {
        return Ok(token_set);
    }
    let Some(saved_refresh_token) = token_set.refresh_token.take() else {
        return Err(ApiError::ExpiredOAuthToken);
    };
    // A branch the endpoint already rejected cannot be revived by trying again.
    // Without this the saved rung spent a doomed round-trip on every credential
    // resolution — the keychain rung has had this guard from the start.
    if let Some(block) = refresh_gate::refresh_blocked(&saved_refresh_token) {
        return Err(saved_refresh_unavailable(block));
    }
    let requested_scopes = std::mem::take(&mut token_set.scopes);
    let refreshed = match refresh(
        config,
        OAuthRefreshRequest::from_config(
            config,
            saved_refresh_token.clone(),
            Some(requested_scopes),
        ),
    ) {
        Ok(refreshed) => {
            refresh_gate::record_success(&saved_refresh_token);
            refreshed
        }
        Err(error) => {
            if refresh_gate::record_failure(&saved_refresh_token, &error) {
                // Once per dead branch, not once per turn: the operator needs the
                // route out, and the raw 400 body does not carry one.
                eprintln!(
                    "\x1b[33mZo's saved Claude login can no longer be refreshed ({error}).\n  \
                     Run `zo login claude` (or /login claude) to reconnect this account.\x1b[0m"
                );
            }
            return Err(error);
        }
    };
    let resolved = OAuthTokenSet {
        access_token: refreshed.access_token,
        refresh_token: refreshed.refresh_token.or(Some(saved_refresh_token)),
        expires_at: refreshed.expires_at,
        scopes: refreshed.scopes,
    };
    save_oauth_credentials(&core_types::OAuthTokenSet {
        access_token: resolved.access_token.clone(),
        refresh_token: resolved.refresh_token.clone(),
        expires_at: resolved.expires_at,
        scopes: resolved.scopes.clone(),
    })
    .map_err(ApiError::from)?;
    Ok(resolved)
}

/// Why the saved rung declined to try, phrased for whoever reads the failure.
fn saved_refresh_unavailable(block: refresh_gate::RefreshBlock) -> ApiError {
    match block {
        refresh_gate::RefreshBlock::Retired => ApiError::Auth(
            "zo's saved Claude login was rejected by the token endpoint and cannot be \
             refreshed — run `zo login claude` to reconnect"
                .to_string(),
        ),
        refresh_gate::RefreshBlock::CoolingDown => ApiError::Auth(
            "zo's saved Claude login could not be refreshed a moment ago; retrying shortly"
                .to_string(),
        ),
    }
}

fn client_runtime_block_on<F, T>(future: F) -> Result<T, ApiError>
where
    F: std::future::Future<Output = Result<T, ApiError>>,
{
    tokio::runtime::Runtime::new()
        .map_err(ApiError::from)?
        .block_on(future)
}

fn load_saved_oauth_token() -> Result<Option<OAuthTokenSet>, ApiError> {
    let token_set = load_oauth_credentials().map_err(ApiError::from)?;
    Ok(token_set.map(|token_set| OAuthTokenSet {
        access_token: token_set.access_token,
        refresh_token: token_set.refresh_token,
        expires_at: token_set.expires_at,
        scopes: token_set.scopes,
    }))
}

fn format_telemetry_usd(amount: f64) -> String {
    let formatted = format_usd(amount);
    if amount.is_sign_positive() && amount > 0.0 && formatted == "$0.0000" {
        "$0.0001".to_string()
    } else {
        formatted
    }
}

fn now_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
fn read_api_key() -> Result<String, ApiError> {
    let auth = AuthSource::from_env_or_saved()?;
    auth.api_key()
        .or_else(|| auth.bearer_token())
        .map(ToOwned::to_owned)
        .ok_or(ApiError::missing_credentials(
            "Anthropic",
            &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
        ))
}

#[cfg(test)]
fn read_auth_token() -> Option<String> {
    read_env_non_empty("ANTHROPIC_AUTH_TOKEN")
        .ok()
        .and_then(std::convert::identity)
}

#[must_use]
pub fn read_base_url() -> String {
    std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

fn request_id_from_headers(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(REQUEST_ID_HEADER)
        .or_else(|| headers.get(ALT_REQUEST_ID_HEADER))
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

/// Parse Anthropic's *unified* rate-limit response headers (the 5-hour and
/// 7-day rolling windows surfaced to subscription / OAuth tokens). Returns
/// `None` when no unified header is present (e.g. api-key requests, which
/// only carry the standard `anthropic-ratelimit-{requests,tokens}-*` set).
fn ratelimit_from_headers(headers: &reqwest::header::HeaderMap) -> Option<RateLimitSnapshot> {
    let snapshot = RateLimitSnapshot {
        five_hour: unified_window(headers, "5h"),
        seven_day: unified_window(headers, "7d"),
        representative: header_trimmed(headers, "anthropic-ratelimit-unified-representative-claim")
            .and_then(|claim| match claim.as_str() {
                "five_hour" => Some(RateLimitWindowKind::FiveHour),
                "seven_day" => Some(RateLimitWindowKind::SevenDay),
                _ => None,
            }),
    };
    snapshot.has_data().then_some(snapshot)
}

fn unified_window(headers: &reqwest::header::HeaderMap, window: &str) -> Option<RateLimitWindow> {
    let utilization = header_trimmed(
        headers,
        &format!("anthropic-ratelimit-unified-{window}-utilization"),
    )?
    .parse::<f64>()
    .ok()?;
    // Prefer a per-window reset header; fall back to the single unified reset
    // (which tracks the representative window) when per-window isn't present.
    let resets_at_unix = header_trimmed(
        headers,
        &format!("anthropic-ratelimit-unified-{window}-reset"),
    )
    .or_else(|| header_trimmed(headers, "anthropic-ratelimit-unified-reset"))
    .and_then(|raw| parse_reset_to_unix(&raw));
    Some(RateLimitWindow {
        utilization,
        resets_at_unix,
    })
}

fn header_trimmed(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
}

/// Parse a rate-limit reset header into Unix epoch seconds. Accepts a bare
/// integer (epoch seconds) or an RFC 3339 timestamp (the format Anthropic's
/// documented `*-reset` headers use).
fn parse_reset_to_unix(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(secs);
    }
    parse_rfc3339_to_unix(raw)
}

/// Minimal RFC 3339 → Unix epoch parser (UTC). Avoids a `chrono` dependency
/// for the one place we need it: `2026-06-01T12:34:56Z` style reset clocks.
/// Offsets other than `Z` are treated as UTC (reset headers use `Z`).
fn parse_rfc3339_to_unix(raw: &str) -> Option<u64> {
    if raw.len() < 19 {
        return None;
    }
    let year: i64 = raw.get(0..4)?.parse().ok()?;
    let month: i64 = raw.get(5..7)?.parse().ok()?;
    let day: i64 = raw.get(8..10)?.parse().ok()?;
    let hour: i64 = raw.get(11..13)?.parse().ok()?;
    let minute: i64 = raw.get(14..16)?.parse().ok()?;
    let second: i64 = raw.get(17..19)?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(secs).ok()
}

/// Days from 1970-01-01 to `y-m-d` (proleptic Gregorian), via Howard
/// Hinnant's `days_from_civil` algorithm.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[derive(Debug)]
pub struct MessageStream {
    request_id: Option<String>,
    rate_limit: Option<RateLimitSnapshot>,
    response: reqwest::Response,
    parser: SseParser,
    pending: VecDeque<StreamEvent>,
    /// Reused scratch for [`SseParser::push_into`] so each chunk parse appends
    /// into one buffer instead of allocating a fresh `Vec` per network read.
    /// Drained into `pending` after every push, so it is empty between chunks
    /// (and across a restart).
    scratch: Vec<StreamEvent>,
    done: bool,
    request: MessageRequest,
    prompt_cache: Option<PromptCache>,
    latest_usage: Option<Usage>,
    usage_recorded: bool,
    last_prompt_cache_record: Arc<Mutex<Option<PromptCacheRecord>>>,
    /// The owning client, kept so a pre-commit transport fault can transparently
    /// re-open the request on a fresh connection (the Messages API has no
    /// resumable-offset token; recovery means re-sending). A cheap clone —
    /// `reqwest::Client` and the cache handles are all `Arc`-backed.
    client: AnthropicClient,
    /// Whether any output the caller can already see has been surfaced this turn.
    /// Once `true` a drop must propagate rather than restart, or the fresh
    /// response would duplicate that output. See `crosses_restart_commit_boundary`.
    committed: bool,
    /// Transparent restarts spent so far, bounded by `client.max_retries`.
    restart_attempts: u32,
    /// When the current restart sequence began, so the whole sequence is bounded
    /// by wall clock and not only by attempt count. `None` until the first
    /// restart. Without it, `max_retries` idle-timeouts (or overload backoffs)
    /// could hold a turn for minutes with nothing on screen — the ceiling the
    /// other streaming backends already carry.
    restart_started_at: Option<Instant>,
}

impl MessageStream {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// Unified rate-limit snapshot parsed from the response headers, if the
    /// server sent the `anthropic-ratelimit-unified-*` set (subscription /
    /// OAuth requests). `None` for api-key requests.
    #[must_use]
    pub fn rate_limit(&self) -> Option<RateLimitSnapshot> {
        self.rate_limit
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        let idle_timeout = super::stream_idle_timeout();
        loop {
            if let Some(event) = self.pending.pop_front() {
                self.observe_event(&event);
                // Past this point the caller has seen output, so a later drop
                // must propagate rather than restart (a fresh stream would
                // duplicate it).
                if super::crosses_restart_commit_boundary(&event) {
                    self.committed = true;
                }
                return Ok(Some(event));
            }

            if self.done {
                let remaining = match self.parser.finish() {
                    Ok(remaining) => remaining,
                    // A trailing frame can carry the provider's error envelope
                    // just as a mid-body one can, so it takes the same gate.
                    Err(error) if self.can_restart(&error) => {
                        self.restart(&error).await?;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                self.pending.extend(remaining);
                if self.pending.is_empty() {
                    return Ok(None);
                }
                continue;
            }

            // Per-chunk idle timeout: each received chunk resets the budget, so a
            // long-but-active stream is never cut, while a silent/half-open
            // connection surfaces a retryable error instead of parking this read
            // forever (which would also stall the caller's wall-clock deadline).
            let read = match idle_timeout {
                Some(idle) => match tokio::time::timeout(idle, self.response.chunk()).await {
                    Ok(chunk) => chunk.map_err(ApiError::from),
                    Err(_elapsed) => Err(ApiError::stream_idle_timeout(idle)),
                },
                None => self.response.chunk().await.map_err(ApiError::from),
            };
            let chunk = match read {
                Ok(chunk) => chunk,
                // Pre-commit transient fault (idle timeout, dropped connection):
                // transparently re-open the same request on a fresh connection.
                // After the commit point we cannot restart without duplicating
                // output, so propagate.
                Err(error) if self.can_restart(&error) => {
                    self.restart(&error).await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            match chunk {
                // A decoded frame can itself BE the failure: Anthropic sheds a
                // streaming request by answering HTTP 200 and then sending
                // `event: error {"type":"overloaded_error"}`, usually before
                // `message_start`. That error surfaces from the parser, not from
                // the body read, so a bare `?` here skipped the restart gate
                // entirely and a recoverable pre-commit overload was promoted
                // into a turn-level failure — five minutes of whole-request
                // retries and, at the end, a dead turn. Route it through the
                // SAME gate as a dropped connection: `can_restart` already
                // demands `error.is_retryable()` (true only for the provider's
                // `overloaded_error` / `rate_limit_error` / `api_error` frames)
                // and `!committed`, so a malformed-JSON or buffer-guard failure
                // still propagates, and a frame that arrives after visible
                // output still propagates rather than duplicating it.
                Some(chunk) => match self.parser.push_into(&chunk, &mut self.scratch) {
                    Ok(()) => self.pending.extend(self.scratch.drain(..)),
                    Err(error) if self.can_restart(&error) => {
                        // Events parsed out of this chunk before the error frame
                        // were never handed to the caller (they are still in
                        // `scratch`, not `pending`), which is exactly why
                        // `committed` is still false — `restart` clears both.
                        // No `continue` needed: this match is the tail of the loop
                        // body, so the fresh stream is polled on the next pass.
                        self.restart(&error).await?;
                    }
                    Err(error) => return Err(error),
                },
                None => {
                    self.done = true;
                }
            }
        }
    }

    /// Whether `error` qualifies for a transparent restart: a retryable transport
    /// / stream fault, with no output yet surfaced and the restart budget intact.
    ///
    /// Bounded by wall clock as well as attempt count: each restart can wait an
    /// idle-timeout or a capacity backoff, so `max_retries` alone let a silent or
    /// shedding backend hold the turn for minutes with nothing on screen. The
    /// ceiling is the same one the OpenAI-compatible and Gemini streams use.
    fn can_restart(&self, error: &ApiError) -> bool {
        super::should_restart_within_budget(
            self.committed,
            error.is_retryable(),
            self.restart_attempts,
            self.client.max_retries,
            self.restart_started_at.as_ref().map(Instant::elapsed),
            super::DEFAULT_MAX_RESTART_WALLCLOCK,
        )
    }

    /// Re-open the stream after a pre-commit fault: jittered backoff, then a fresh
    /// `messages` request whose response/parser replace the dead ones so the loop
    /// resumes from a clean turn. Any partial bytes in the old parser are dropped
    /// with it — safe because nothing has been surfaced.
    ///
    /// Still silent on stderr — writing there while the TUI holds an alt-screen
    /// produces the "staircase" (see `expect_success`) — but no longer silent to
    /// the *user*: the installed retry-notice callback gets the same row the
    /// establish-time ladder emits, so a multi-second reconnect reads as
    /// "provider overloaded; retrying in 2s" instead of a frozen spinner.
    async fn restart(&mut self, cause: &ApiError) -> Result<(), ApiError> {
        self.restart_attempts += 1;
        let base = self.client.backoff_for_attempt(self.restart_attempts)?;
        let delay = super::retry_backoff::spread_backoff(base);
        if let Some(callback) = &self.client.retry_notice {
            callback(AnthropicRetryNotice {
                attempt: self.restart_attempts,
                max_attempts: self.client.max_retries,
                delay,
                error: cause.to_string(),
            });
        }
        // Start the wall-clock budget at the FIRST restart of the sequence; a
        // successful event resets neither (a stream that keeps faulting after
        // partial progress is exactly the case the ceiling exists for).
        self.restart_started_at.get_or_insert_with(Instant::now);
        tokio::time::sleep(delay).await;
        let response = self
            .client
            .send_with_retry(&self.request.clone().with_streaming())
            .await?;
        self.rate_limit = ratelimit_from_headers(response.headers());
        if let Some(snapshot) = self.rate_limit {
            crate::quota::record_rate_limit_snapshot(snapshot);
        }
        self.request_id = request_id_from_headers(response.headers());
        self.response = response;
        self.parser = SseParser::new();
        self.pending.clear();
        self.scratch.clear();
        self.done = false;
        // `latest_usage` / `usage_recorded` / `committed` are intentionally NOT
        // reset: a restart only happens pre-commit, so `committed` is still false
        // and no caller-visible usage has been recorded yet. The fresh turn's own
        // `message_delta` overwrites `latest_usage`, so the aborted attempt's
        // bookkeeping cannot double-count. Resetting them here would be redundant.
        Ok(())
    }

    fn observe_event(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::MessageDelta(MessageDeltaEvent {
                usage,
                context_management,
                ..
            }) => {
                self.latest_usage = Some(*usage);
                if let Some(event) = context_edit_analytics_event(
                    self.request_id.as_deref(),
                    context_management.as_ref(),
                ) {
                    if let Some(session_tracer) = &self.client.session_tracer {
                        session_tracer.record_analytics(event);
                    }
                }
            }
            StreamEvent::MessageStop(_) => {
                if !self.usage_recorded {
                    if let (Some(prompt_cache), Some(usage)) =
                        (&self.prompt_cache, self.latest_usage.as_ref())
                    {
                        let record = prompt_cache.record_usage(&self.request, usage);
                        *self
                            .last_prompt_cache_record
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(record);
                    }
                    self.usage_recorded = true;
                }
            }
            _ => {}
        }
    }
}

async fn expect_success(response: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs);

    // NOTE: 429 diagnostic `eprintln!` blocks were removed here.
    // Writing to stderr while the TUI holds an alt-screen + raw-mode
    // terminal produces a "staircase" because stderr is a separate FD
    // from the ratatui stdout handle and ONLCR is disabled, so every
    // embedded `\n` becomes a bare LF. The 429 error is already
    // propagated via `Err(ApiError::Api { .. })` and rendered as a
    // `RenderBlock::System` by the TUI — no direct stderr write needed.

    let body = response.text().await.unwrap_or_else(|_| String::new());
    let parsed_error = serde_json::from_str::<AnthropicErrorEnvelope>(&body).ok();
    let retryable = is_retryable_status(status);

    Err(ApiError::Api {
        status,
        error_type: parsed_error
            .as_ref()
            .map(|error| error.error.error_type.clone()),
        message: parsed_error
            .as_ref()
            .map(|error| error.error.message.clone()),
        body,
        retryable,
        retry_after,
    })
}

const fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    // Mirror the Anthropic SDK's `shouldRetry`: request timeout (408), conflict
    // (409), rate limit (429), and *every* server error (>= 500) — which
    // crucially includes 529 `overloaded_error`, Anthropic's transient overload
    // signal. A fixed 5xx whitelist used to omit 529 (and 501/505/511), so an
    // overload bubbled straight up to the user instead of being retried like the
    // official client does.
    let code = status.as_u16();
    matches!(code, 408 | 409 | 429) || code >= 500
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorEnvelope {
    error: AnthropicErrorBody,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorBody {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

#[cfg(test)]
mod tests;
