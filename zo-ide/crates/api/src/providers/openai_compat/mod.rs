use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use super::{crosses_restart_commit_boundary, prompt_cache_key, read_env_non_empty};

use crate::error::ApiError;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    ImageSource, InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent, MessageRequest,
    MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock, ReasoningRequest,
    StreamEvent, ToolChoice, ToolDefinition, ToolLedgerView, Usage,
};

use super::PromptCacheStrategy;

pub const DEFAULT_XAI_BASE_URL: &str = "https://api.x.ai/v1";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
const REQUEST_ID_HEADER: &str = "request-id";
const ALT_REQUEST_ID_HEADER: &str = "x-request-id";
const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(200);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(2);
const DEFAULT_MAX_RETRIES: u32 = 2;

/// `User-Agent` presented by OpenAI's Codex CLI. Gateways that only accept a
/// whitelisted client wire image (rejecting a generic client with a 401
/// `unauthorized_client_error`) match on this fingerprint; a custom provider
/// opts in with `"client_fingerprint": "codex"`.
const CODEX_CLIENT_USER_AGENT: &str = "codex_cli_rs/0.144.1 (Mac OS; arm64)";
/// `User-Agent` presented by Anthropic's Claude Code CLI, for
/// `"client_fingerprint": "claude-code"` — the same identity the Anthropic
/// road presents, so the version is the installed CLI's (or the one the API
/// asked for), never a number pinned here. Leaked once: the preset table
/// hands out `&'static str`.
fn claude_code_client_user_agent() -> &'static str {
    static PRESENTED: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    PRESENTED.get_or_init(|| leak(&telemetry::ClientIdentity::default().user_agent()))
}

/// Resolve a named client-fingerprint preset to the `User-Agent` that client
/// presents. Unknown names yield `None` so a typo degrades to no override
/// rather than a silently wrong fingerprint.
fn client_fingerprint_user_agent(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "codex" | "codex-cli" | "codex_cli_rs" => Some(CODEX_CLIENT_USER_AGENT),
        "claude-code" | "claude" | "claude-cli" => Some(claude_code_client_user_agent()),
        _ => None,
    }
}

// Each bool is an independent per-endpoint wire capability, not a state
// machine: grouping them into an enum would not remove a single combination.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenAiCompatConfig {
    pub provider_name: &'static str,
    pub api_key_env: &'static str,
    pub base_url_env: &'static str,
    pub default_base_url: &'static str,
    pub credential_env_vars: &'static [&'static str],
    /// Whether streaming requests should include OpenAI's usage opt-in.
    /// Some compatible providers reject `stream_options`; keep this data-driven
    /// per endpoint instead of keying behavior off provider name strings.
    pub request_stream_usage: bool,
    /// Whether this provider reports cache-read token counts in its usage
    /// payload. OpenAI exposes prompt-cache hits via
    /// `prompt_tokens_details.cached_tokens`; compatible providers that do not
    /// expose that field keep this `false` so cache figures stay 0 rather than
    /// being fabricated.
    pub supports_cache_tokens: bool,
    /// Provider-specific prompt-cache request strategy. Official OpenAI accepts
    /// `prompt_cache_key`; extended retention is model-gated. Compatible
    /// providers generally reject unknown fields and keep request controls off.
    pub prompt_cache_strategy: PromptCacheStrategy,
    /// Whether this endpoint accepts multimodal `image_url` content parts. When
    /// `false`, a user message's images are degraded to a text placeholder
    /// instead of being lowered into the request, because vision-less servers
    /// (e.g. `DeepSeek`) reject the `image_url` variant with a hard 400 —
    /// `unknown variant image_url, expected text`. First-party OpenAI/xAI/Google
    /// endpoints set this `true`; self-hosted and custom providers default off
    /// and opt in via `CustomProviderConfig::supports_vision`.
    pub supports_vision: bool,
    /// Optional `User-Agent` override. Some gateways only accept requests whose
    /// wire image matches a whitelisted client (Codex CLI / Claude Code), so a
    /// custom provider presents that client's fingerprint via
    /// `CustomProviderConfig::client_fingerprint` / `user_agent`. `None`
    /// leaves the default client image unchanged for built-in providers.
    pub user_agent: Option<&'static str>,
    /// Extra request headers applied verbatim, for gateways that gate on a
    /// client fingerprint beyond the `User-Agent`. Empty for built-in providers.
    pub client_headers: &'static [(&'static str, &'static str)],
    /// Whether this endpoint's models take OpenAI's `reasoning_effort` field.
    ///
    /// Built-in providers leave this `false` and rely on `is_reasoning_model`,
    /// which recognizes OpenAI's own reasoning families by id (`o1`/`o3`/`o4`/
    /// `gpt-5`). That id test cannot recognize a third-party reasoning model
    /// served through a gateway (`kimi-*`, `deepseek-r*`, `glm-*`, a gateway
    /// alias like `default`), so those requests silently shipped with NO effort
    /// field at all — `/effort` was inert for them, dynamic band included.
    /// Custom providers opt in via
    /// `CustomProviderConfig::supports_reasoning_effort`; it stays off by
    /// default because a non-reasoning endpoint 400s on the unknown field.
    pub supports_reasoning_effort: bool,
}

const XAI_ENV_VARS: &[&str] = &["XAI_API_KEY"];
const OPENAI_ENV_VARS: &[&str] = &["OPENAI_API_KEY"];
const GOOGLE_ENV_VARS: &[&str] = &["GOOGLE_API_KEY"];
const OLLAMA_ENV_VARS: &[&str] = &[];

impl OpenAiCompatConfig {
    #[must_use]
    pub const fn xai() -> Self {
        Self {
            provider_name: "xAI",
            api_key_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: DEFAULT_XAI_BASE_URL,
            credential_env_vars: XAI_ENV_VARS,
            request_stream_usage: false,
            supports_cache_tokens: false,
            prompt_cache_strategy: PromptCacheStrategy::NoRequestControls,
            supports_vision: true,
            user_agent: None,
            client_headers: &[],
            supports_reasoning_effort: false,
        }
    }

    #[must_use]
    pub const fn openai() -> Self {
        Self {
            provider_name: "OpenAI",
            api_key_env: "OPENAI_API_KEY",
            base_url_env: "OPENAI_BASE_URL",
            default_base_url: DEFAULT_OPENAI_BASE_URL,
            credential_env_vars: OPENAI_ENV_VARS,
            request_stream_usage: true,
            supports_cache_tokens: true,
            prompt_cache_strategy: PromptCacheStrategy::OpenAiPromptCacheKey,
            supports_vision: true,
            user_agent: None,
            client_headers: &[],
            supports_reasoning_effort: false,
        }
    }
    #[must_use]
    pub const fn google() -> Self {
        Self {
            provider_name: "Google",
            api_key_env: "GOOGLE_API_KEY",
            base_url_env: "GOOGLE_BASE_URL",
            default_base_url: DEFAULT_GOOGLE_BASE_URL,
            credential_env_vars: GOOGLE_ENV_VARS,
            request_stream_usage: false,
            supports_cache_tokens: false,
            prompt_cache_strategy: PromptCacheStrategy::NoRequestControls,
            supports_vision: true,
            user_agent: None,
            client_headers: &[],
            supports_reasoning_effort: false,
        }
    }

    #[must_use]
    pub const fn ollama() -> Self {
        Self {
            provider_name: "Ollama",
            api_key_env: "OLLAMA_API_KEY",
            base_url_env: "OLLAMA_BASE_URL",
            default_base_url: DEFAULT_OLLAMA_BASE_URL,
            credential_env_vars: OLLAMA_ENV_VARS,
            request_stream_usage: false,
            supports_cache_tokens: false,
            prompt_cache_strategy: PromptCacheStrategy::NoRequestControls,
            supports_vision: false,
            user_agent: None,
            client_headers: &[],
            supports_reasoning_effort: false,
        }
    }

    /// Build a process-lifetime config for a user-defined
    /// OpenAI-compatible endpoint. The endpoint owns model routing elsewhere
    /// (for example `CustomProviderConfig::models`); this keeps the HTTP
    /// client focused on auth and URL behavior.
    #[must_use]
    pub fn from_user(
        provider_name: &str,
        base_url: &str,
        api_key_env: Option<&str>,
        request_stream_usage: bool,
    ) -> Self {
        let api_key_env = api_key_env.map_or("", leak);
        Self {
            provider_name: leak(provider_name),
            api_key_env,
            base_url_env: "",
            default_base_url: leak(base_url),
            credential_env_vars: env_var_slice(api_key_env),
            request_stream_usage,
            supports_cache_tokens: false,
            prompt_cache_strategy: PromptCacheStrategy::NoRequestControls,
            // Custom/self-hosted endpoints default vision OFF: a server that
            // does not accept `image_url` parts hard-400s the whole request, so
            // the safe default degrades images to text. A vision-capable custom
            // endpoint opts back in via [`CustomProviderConfig::supports_vision`]
            // (applied in [`CustomProviderConfig::to_static_config`]).
            supports_vision: false,
            user_agent: None,
            client_headers: &[],
            supports_reasoning_effort: false,
        }
    }

    #[must_use]
    pub const fn credential_env_vars(self) -> &'static [&'static str] {
        self.credential_env_vars
    }

    #[must_use]
    pub fn requires_auth(self) -> bool {
        self.provider_name != "Ollama"
    }

    /// Maker attribution for the shared non-Anthropic identity override, or
    /// `None` when zo must not assert a specific maker. The three first-party
    /// OpenAI-compatible providers name their maker; `Ollama` and any
    /// user-defined ([`Self::from_user`]) provider return `None` so a
    /// self-hosted endpoint is identity-corrected without being mislabeled.
    #[must_use]
    pub fn identity_maker(self) -> Option<&'static str> {
        match self.provider_name {
            "OpenAI" => Some("OpenAI"),
            "xAI" => Some("xAI"),
            "Google" => Some("Google"),
            _ => None,
        }
    }
}

/// A user-defined OpenAI-compatible provider, expressed as data
/// (`ZO_CUSTOM_PROVIDERS`) instead of a compiled `MODEL_REGISTRY` row. The
/// owned `String` fields mirror [`OpenAiCompatConfig`]'s `&'static str` ones;
/// [`Self::to_static_config`] bridges them.
// Mirrors [`OpenAiCompatConfig`]: independent operator-declared capabilities,
// each deserialized straight from a settings key.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Deserialize)]
pub struct CustomProviderConfig {
    /// Display name; also the provider tag in error messages.
    pub name: String,
    /// OpenAI-compatible endpoint base URL (e.g. `https://host/v1`).
    pub base_url: String,
    /// Env var holding the API key. `None` pairs with `requires_auth: false`
    /// for keyless self-hosted endpoints.
    #[serde(default)]
    pub auth_env: Option<String>,
    /// Models served by this provider; a `--model` value matching one's id
    /// (case-insensitively) routes here. Each entry is a plain id or an object
    /// carrying that model's own limits (see [`CustomModel`]).
    #[serde(default)]
    pub models: Vec<CustomModel>,
    /// Whether a key is mandatory. `true` (the default) errors clearly when the
    /// `auth_env` var is unset; `false` lets a keyless self-host build.
    #[serde(default = "default_requires_auth")]
    pub requires_auth: bool,
    /// Whether streaming requests should ask for usage chunks through
    /// `stream_options.include_usage`. Disable for compatible servers that do
    /// not accept the OpenAI usage opt-in.
    #[serde(default = "default_include_usage")]
    pub include_usage: bool,
    /// Optional context-window override for every model declared by this custom
    /// provider that states none of its own (a [`CustomModel`]'s own value
    /// wins). OpenAI-compatible `/models` responses usually do not expose
    /// token limits, so custom endpoints need a persistent settings-file escape
    /// hatch instead of an env-only [`crate::providers::MODEL_CONTEXT_WINDOWS_ENV`].
    #[serde(default)]
    pub context_window: Option<u64>,
    /// Optional max-output-token override for every model declared by this
    /// custom provider that states none of its own. Used by
    /// [`crate::providers::max_tokens_for_model`].
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    /// Whether this endpoint accepts multimodal `image_url` content parts.
    /// Defaults `false`: a vision-less OpenAI-compatible server (e.g. a
    /// DeepSeek-style gateway) rejects `image_url` with a hard 400, so images
    /// are degraded to a text placeholder unless the operator opts in here.
    #[serde(default)]
    pub supports_vision: bool,
    /// Read-only advisory VRAM estimate for local models. Zo displays this
    /// only when the `model-fit-hints` feature is enabled; it never selects or
    /// serves a model from the estimate.
    #[serde(default)]
    pub estimated_vram_gb: Option<u16>,
    /// Quantization label paired with [`Self::estimated_vram_gb`], such as
    /// `Q4_K_M`. Ignored when no estimate is present.
    #[serde(default)]
    pub quantization: Option<String>,
    /// Named client-fingerprint preset. `"codex"` or `"claude-code"` make zo
    /// present that client's `User-Agent`, for gateways that only accept a
    /// whitelisted client wire image (a 401 `unauthorized_client_error`
    /// otherwise). A raw [`Self::user_agent`] overrides the preset.
    #[serde(default)]
    pub client_fingerprint: Option<String>,
    /// Raw `User-Agent` override, sent verbatim. Wins over
    /// [`Self::client_fingerprint`] when both are set.
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Extra request headers sent verbatim, for gateways that gate on more than
    /// the `User-Agent`.
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    /// Whether this endpoint's models accept OpenAI's `reasoning_effort` field.
    ///
    /// Zo recognizes OpenAI's own reasoning families by model id, but a
    /// third-party reasoning model reached through a gateway (`kimi-*`,
    /// `deepseek-r*`, `glm-*`, or a gateway alias such as `default`) matches no
    /// such id, so its requests carried no effort field and `/effort` — the
    /// Smart dynamic band included — had no effect on them. Set this `true` for
    /// an endpoint whose models do take `reasoning_effort`. Left `false` (the
    /// default) for endpoints that would 400 on the unknown field.
    #[serde(default)]
    pub supports_reasoning_effort: bool,
}

/// One model's own limits, where it states them; `None` defers to the
/// provider-wide value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelLimits {
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

impl ModelLimits {
    /// Whether any limit is stated (a zero states nothing).
    #[must_use]
    pub fn is_stated(self) -> bool {
        self.context_window.is_some_and(|value| value > 0)
            || self.max_output_tokens.is_some_and(|value| value > 0)
    }
}

/// One entry of a custom provider's `models`: a plain id (`"vendor/x"`), or
/// an object naming the id with that model's own limits —
/// `{"id": "vendor/x", "context_window": 1048576, "max_output_tokens": 65536}`.
/// Both shapes parse into this one type, so a file of plain ids keeps working
/// and a router that read each model's window off its `/models` answer can say
/// so per model instead of one number for all of them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "CustomModelWire")]
pub struct CustomModel {
    pub id: String,
    pub limits: ModelLimits,
}

#[derive(Deserialize)]
#[serde(
    untagged,
    expecting = "a model id, or an object {\"id\", \"context_window\", \"max_output_tokens\"}"
)]
enum CustomModelWire {
    Id(String),
    Stated {
        id: String,
        #[serde(default)]
        context_window: Option<u64>,
        #[serde(default)]
        max_output_tokens: Option<u64>,
    },
}

impl From<CustomModelWire> for CustomModel {
    fn from(wire: CustomModelWire) -> Self {
        match wire {
            CustomModelWire::Id(id) => Self {
                id,
                limits: ModelLimits::default(),
            },
            CustomModelWire::Stated {
                id,
                context_window,
                max_output_tokens,
            } => Self {
                id,
                limits: ModelLimits {
                    context_window,
                    max_output_tokens,
                },
            },
        }
    }
}

const fn default_requires_auth() -> bool {
    true
}

const fn default_include_usage() -> bool {
    true
}

/// Leak an owned string to `&'static str`. Sound for process-lifetime config:
/// the custom catalog is parsed once at process start and lives forever.
fn leak(value: &str) -> &'static str {
    Box::leak(value.to_owned().into_boxed_str())
}

fn env_var_slice(value: &'static str) -> &'static [&'static str] {
    if value.is_empty() {
        &[]
    } else {
        Box::leak(vec![value].into_boxed_slice())
    }
}

impl CustomProviderConfig {
    /// Bridge to the `&'static str`-based [`OpenAiCompatConfig`] the client
    /// consumes, leaking the strings once. `base_url_env` is left empty so the
    /// client uses `base_url` directly with no env indirection.
    #[must_use]
    pub fn to_static_config(&self) -> OpenAiCompatConfig {
        let mut config = OpenAiCompatConfig::from_user(
            &self.name,
            &self.base_url,
            self.auth_env.as_deref(),
            self.include_usage,
        );
        // Operator opt-in: a vision-capable custom endpoint re-enables `image_url`
        // parts that `from_user` conservatively disabled.
        config.supports_vision = self.supports_vision;
        // Same shape of opt-in for the effort field: without it a gateway-served
        // reasoning model gets no `reasoning_effort` at all.
        config.supports_reasoning_effort = self.supports_reasoning_effort;
        config.user_agent = self.resolve_user_agent();
        config.client_headers = self.resolve_client_headers();
        config
    }

    /// Resolve the effective `User-Agent`: a raw [`Self::user_agent`] wins, then
    /// a named [`Self::client_fingerprint`] preset, else `None`.
    fn resolve_user_agent(&self) -> Option<&'static str> {
        if let Some(user_agent) = self
            .user_agent
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(leak(user_agent));
        }
        self.client_fingerprint
            .as_deref()
            .and_then(client_fingerprint_user_agent)
    }

    /// Leak the operator-declared extra headers into the process-lifetime slice
    /// the client sends. Header names are trimmed; blank names are dropped.
    fn resolve_client_headers(&self) -> &'static [(&'static str, &'static str)] {
        let Some(headers) = self.headers.as_ref() else {
            return &[];
        };
        let pairs: Vec<(&'static str, &'static str)> = headers
            .iter()
            .filter(|(name, _)| !name.trim().is_empty())
            .map(|(name, value)| (leak(name.trim()), leak(value.as_str())))
            .collect();
        if pairs.is_empty() {
            &[]
        } else {
            Box::leak(pairs.into_boxed_slice())
        }
    }

    #[must_use]
    pub fn to_fit_hint(&self) -> Option<super::ModelFitHint> {
        self.estimated_vram_gb.map(|estimated_vram_gb| {
            super::ModelFitHint::new(
                estimated_vram_gb,
                self.quantization.as_deref().map_or("unknown", leak),
            )
        })
    }
}

#[derive(Clone)]
enum OpenAiCompatAuth {
    /// No authentication header (custom/self-hosted endpoints only).
    None,
    /// Static bearer value: OpenAI/xAI API keys and Gemini API keys all use the
    /// OpenAI-compatible `Authorization: Bearer ...` convention.
    StaticBearer(String),
    /// Resolve a fresh Google OAuth access token for Gemini requests from ADC
    /// or gcloud, using the cache in `google_auth` to avoid per-request mints.
    GoogleGeminiOAuth,
}

/// A client's debug line names how it authenticates, never with what: a
/// bearer is a key or a login token, and a `{client:?}` in a panic or a log
/// must not carry it (a test's expect message once did).
/// Whether a client authenticates with exactly `bearer` — the answer a test
/// needs from a client whose debug line names no secret.
#[cfg(test)]
impl OpenAiCompatClient {
    #[must_use]
    pub(crate) fn speaks_with_bearer(&self, bearer: &str) -> bool {
        matches!(&self.auth, OpenAiCompatAuth::StaticBearer(held) if held == bearer)
    }
}

impl std::fmt::Debug for OpenAiCompatAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::StaticBearer(_) => f.write_str("StaticBearer(<redacted>)"),
            Self::GoogleGeminiOAuth => f.write_str("GoogleGeminiOAuth"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatClient {
    http: reqwest::Client,
    auth: OpenAiCompatAuth,
    config: OpenAiCompatConfig,
    base_url: String,
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl OpenAiCompatClient {
    const fn config(&self) -> OpenAiCompatConfig {
        self.config
    }
    #[must_use]
    pub fn new(api_key: impl Into<String>, config: OpenAiCompatConfig) -> Self {
        Self::new_with_auth(OpenAiCompatAuth::StaticBearer(api_key.into()), config)
    }

    fn new_with_auth(auth: OpenAiCompatAuth, config: OpenAiCompatConfig) -> Self {
        Self {
            http: super::shared_http_client(),
            auth,
            config,
            base_url: read_base_url(config),
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
        }
    }

    #[must_use]
    pub fn google_oauth(config: OpenAiCompatConfig) -> Self {
        Self::new_with_auth(OpenAiCompatAuth::GoogleGeminiOAuth, config)
    }

    pub fn from_env(config: OpenAiCompatConfig) -> Result<Self, ApiError> {
        let Some(api_key) = read_api_key(config.api_key_env)? else {
            return Err(ApiError::missing_credentials(
                config.provider_name,
                config.credential_env_vars(),
            ));
        };
        Ok(Self::new(api_key, config))
    }

    pub fn from_env_optional_auth(config: OpenAiCompatConfig) -> Result<Self, ApiError> {
        let auth = read_env_non_empty(config.api_key_env)?
            .or_else(|| read_saved_api_key_non_empty(config.api_key_env))
            .map_or(OpenAiCompatAuth::None, OpenAiCompatAuth::StaticBearer);
        Ok(Self::new_with_auth(auth, config))
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
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

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        let request = MessageRequest {
            stream: false,
            ..request.clone()
        };
        let response = self.send_with_retry(&request).await?;
        let request_id = request_id_from_headers(response.headers());
        let payload = response.json::<ChatCompletionResponse>().await?;
        let config = request_config_for_base_url(self.config(), &self.base_url);
        let mut normalized = normalize_response(&request.model, payload, config)?;
        if normalized.request_id.is_none() {
            normalized.request_id = request_id;
        }
        Ok(normalized)
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        let response = self
            .send_with_retry(&request.clone().with_streaming())
            .await?;
        Ok(MessageStream {
            request_id: request_id_from_headers(response.headers()),
            response,
            parser: OpenAiSseParser::new(),
            pending: VecDeque::new(),
            done: false,
            state: StreamState::new(
                request.model.clone(),
                request_config_for_base_url(self.config(), &self.base_url),
            ),
            client: self.clone(),
            request: request.clone().with_streaming(),
            committed: false,
            restart_attempts: 0,
            restart_window_start: None,
            max_restart_wallclock: super::DEFAULT_MAX_RESTART_WALLCLOCK,
            retry_notice: super::StreamRetryNotifier::none(),
        })
    }

    async fn send_with_retry(
        &self,
        request: &MessageRequest,
    ) -> Result<reqwest::Response, ApiError> {
        let mut attempts = 0;

        let last_error = loop {
            attempts += 1;
            let retryable_error = match self.send_raw_request(request).await {
                Ok(response) => match expect_success(response).await {
                    Ok(response) => return Ok(response),
                    Err(error) if error.is_retryable() && attempts <= self.max_retries + 1 => error,
                    Err(error) => return Err(error),
                },
                Err(error) if error.is_retryable() && attempts <= self.max_retries + 1 => error,
                Err(error) => return Err(error),
            };

            if attempts > self.max_retries {
                break retryable_error;
            }

            tokio::time::sleep(super::retry_backoff::spread_backoff(
                self.backoff_for_attempt(attempts)?,
            ))
            .await;
        };

        Err(ApiError::RetriesExhausted {
            attempts,
            last_error: Box::new(last_error),
        })
    }

    async fn send_raw_request(
        &self,
        request: &MessageRequest,
    ) -> Result<reqwest::Response, ApiError> {
        let request_url = chat_completions_endpoint(&self.base_url);
        let config = request_config_for_base_url(self.config(), &self.base_url);
        let builder = self
            .http
            .post(&request_url)
            .header("content-type", "application/json")
            .json(&build_chat_completion_request(request, config));
        let builder = self.apply_client_identity(builder);
        let builder = self.apply_auth(builder).await?;
        builder.send().await.map_err(ApiError::from)
    }

    /// Present the operator-configured client fingerprint (`User-Agent` and any
    /// extra headers) so gateways that whitelist a specific client wire image
    /// accept the request. A no-op for built-in providers, which carry none.
    fn apply_client_identity(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(user_agent) = self.config.user_agent {
            builder = builder.header("user-agent", user_agent);
        }
        for (name, value) in self.config.client_headers {
            builder = builder.header(*name, *value);
        }
        builder
    }

    async fn apply_auth(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, ApiError> {
        match &self.auth {
            OpenAiCompatAuth::None => Ok(builder),
            OpenAiCompatAuth::StaticBearer(token) if token.is_empty() => Ok(builder),
            OpenAiCompatAuth::StaticBearer(token) => Ok(builder.bearer_auth(token)),
            OpenAiCompatAuth::GoogleGeminiOAuth => {
                let token = super::google_auth::gemini_access_token().await?;
                let builder = builder.bearer_auth(token);
                Ok(
                    if let Some(project) = super::google_auth::request_user_project() {
                        builder.header("x-goog-user-project", project)
                    } else {
                        builder
                    },
                )
            }
        }
    }

    fn backoff_for_attempt(&self, attempt: u32) -> Result<Duration, ApiError> {
        super::backoff_for_attempt(attempt, self.initial_backoff, self.max_backoff)
    }
}

#[derive(Debug)]
pub struct MessageStream {
    request_id: Option<String>,
    response: reqwest::Response,
    parser: OpenAiSseParser,
    pending: VecDeque<StreamEvent>,
    done: bool,
    state: StreamState,
    client: OpenAiCompatClient,
    request: MessageRequest,
    committed: bool,
    restart_attempts: u32,
    /// Start of the restart sequence (first restart), for the wall-clock
    /// ceiling in [`super::should_restart_within_budget`].
    restart_window_start: Option<std::time::Instant>,
    /// Total wall-clock ceiling over the restart sequence. Constant in
    /// production ([`super::DEFAULT_MAX_RESTART_WALLCLOCK`]); a field so tests
    /// can exercise the reopen-timeout path without multi-minute waits.
    max_restart_wallclock: Duration,
    /// Optional sink fired just before each transparent restart sleeps, so a
    /// live UI can show the reconnect pause instead of a freeze.
    retry_notice: super::StreamRetryNotifier,
}

impl MessageStream {
    /// Install a mid-stream restart notice sink. `None` by default: notices
    /// are dropped, preserving log-only behaviour for non-interactive callers.
    #[must_use]
    pub fn with_retry_notice_callback(
        mut self,
        callback: impl Fn(core_types::StreamRetryNotice) + Send + Sync + 'static,
    ) -> Self {
        self.retry_notice.install(callback);
        self
    }

    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        let idle_timeout = super::stream_idle_timeout();
        loop {
            if let Some(event) = self.pending.pop_front() {
                if crosses_restart_commit_boundary(&event) {
                    self.committed = true;
                }
                return Ok(Some(event));
            }

            if self.done {
                self.pending.extend(self.state.finish()?);
                if let Some(event) = self.pending.pop_front() {
                    return Ok(Some(event));
                }
                return Ok(None);
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
                Err(error) if self.can_restart(&error) => {
                    self.restart(&error).await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            match chunk {
                // A decoded frame can itself BE the failure: a gateway that dies
                // after the response headers are out sends `{"error": {...}}` where
                // a chunk belongs ([`parse_stream_error_frame`], which already
                // computes `retryable` from the shared capacity vocabulary). That
                // error surfaces from the parser rather than the body read, so a
                // bare `?` here skipped the restart gate entirely and a recoverable
                // pre-commit capacity refusal became a turn-level failure — the same
                // defect the Anthropic lane carried. The gate itself decides: it
                // demands `error.is_retryable()` and `!committed`, so a
                // malformed-payload or context-length failure still propagates, and
                // a frame arriving after visible output still propagates rather than
                // duplicating it. `ingest_chunk` keeps its `?`: the state machine
                // only ever fails on protocol shape, never on capacity, so routing
                // it through a gate that demands retryability would be dead code.
                Some(chunk) => match self.parser.push(&chunk) {
                    Ok(parsed) => {
                        for parsed in parsed {
                            self.pending.extend(self.state.ingest_chunk(parsed)?);
                        }
                    }
                    Err(error) if self.can_restart(&error) => {
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

    fn can_restart(&self, error: &ApiError) -> bool {
        super::should_restart_within_budget(
            self.committed,
            error.is_retryable(),
            self.restart_attempts,
            self.client.max_retries,
            self.restart_window_start.map(|start| start.elapsed()),
            self.max_restart_wallclock,
        )
    }

    async fn restart(&mut self, last_error: &ApiError) -> Result<(), ApiError> {
        let window_start = *self
            .restart_window_start
            .get_or_insert_with(std::time::Instant::now);
        self.restart_attempts += 1;
        let base = self.client.backoff_for_attempt(self.restart_attempts)?;
        let delay = super::retry_backoff::spread_backoff(base);
        // Surface the otherwise-silent reconnect pause to a live UI so it
        // reads as "reconnecting", not a freeze. Same classifier label as the
        // establish-time retry notice so the wording stays in lockstep.
        self.retry_notice.notify(core_types::StreamRetryNotice {
            kind: core_types::StreamNoticeKind::Reconnect,
            label: core_types::retry_signal::retry_notice_label(&last_error.to_string()),
            attempt: self.restart_attempts,
            max_attempts: self.client.max_retries,
            delay,
        });
        tokio::time::sleep(delay).await;
        // The shared HTTP client deliberately carries no blanket request
        // timeout, so a reconnect the server accepts but never answers would
        // park this await forever — and the between-attempts wall-clock gate
        // in `can_restart` would never run again. Bound the reopen by the
        // budget remaining in the restart window instead.
        let remaining = self
            .max_restart_wallclock
            .saturating_sub(window_start.elapsed());
        let response =
            match tokio::time::timeout(remaining, self.client.send_with_retry(&self.request)).await
            {
                Ok(response) => response?,
                Err(_elapsed) => return Err(ApiError::stream_restart_timeout(remaining)),
            };
        self.request_id = request_id_from_headers(response.headers());
        self.response = response;
        self.parser = OpenAiSseParser::new();
        self.pending.clear();
        self.done = false;
        self.state = StreamState::new(
            self.request.model.clone(),
            request_config_for_base_url(self.client.config(), &self.client.base_url),
        );
        Ok(())
    }
}

#[derive(Debug, Default)]
struct OpenAiSseParser {
    buffer: Vec<u8>,
    // Resume separator scans across pushes. Large provider frames can arrive
    // split across many chunks; rescanning the whole buffer on every push is
    // quadratic and can make the TUI appear frozen before a burst of output.
    scanned: usize,
}

impl OpenAiSseParser {
    fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<ChatCompletionChunk>, ApiError> {
        // Bound the retained partial-frame buffer with the crate-wide SSE cap so
        // a stream that never emits a separator cannot grow memory without limit.
        crate::sse::guard_sse_buffer_push(self.buffer.len(), chunk.len())?;
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some(frame) = next_sse_frame(&mut self.buffer, &mut self.scanned) {
            if let Some(event) = parse_sse_frame(&frame)? {
                events.push(event);
            }
        }

        Ok(events)
    }
}

/// Content-block index reserved for the coalesced reasoning/thinking block
/// (`reasoning_content` from DeepSeek-reasoner and other OpenAI-compatible
/// reasoners). `u32::MAX` so it never collides with the answer-text block
/// (index 0) or tool-call blocks (`openai_index + 1`).
const OPENAI_REASONING_BLOCK_INDEX: u32 = u32::MAX;

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
struct StreamState {
    config: OpenAiCompatConfig,
    model: String,
    message_started: bool,
    text_started: bool,
    text_finished: bool,
    reasoning_started: bool,
    reasoning_finished: bool,
    finished: bool,
    stop_reason: Option<String>,
    usage: Option<Usage>,
    tool_calls: BTreeMap<u32, ToolCallState>,
}

impl StreamState {
    fn new(model: String, config: OpenAiCompatConfig) -> Self {
        Self {
            config,
            model,
            message_started: false,
            text_started: false,
            text_finished: false,
            reasoning_started: false,
            reasoning_finished: false,
            finished: false,
            stop_reason: None,
            usage: None,
            tool_calls: BTreeMap::new(),
        }
    }

    /// Emit a streamed reasoning/thinking delta, opening the coalesced reasoning
    /// block on first use. Mirrors the answer-text path but on
    /// [`OPENAI_REASONING_BLOCK_INDEX`].
    fn push_reasoning_delta(&mut self, events: &mut Vec<StreamEvent>, reasoning: &str) {
        if !self.reasoning_started {
            self.reasoning_started = true;
            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index: OPENAI_REASONING_BLOCK_INDEX,
                content_block: OutputContentBlock::Thinking {
                    thinking: String::new(),
                    signature: None,
                },
            }));
        }
        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: OPENAI_REASONING_BLOCK_INDEX,
            delta: ContentBlockDelta::ThinkingDelta {
                thinking: reasoning.to_string(),
            },
        }));
    }

    /// Close the reasoning block if it is open — called when the answer begins
    /// and at stream end so reasoning never lingers `done:false` above later
    /// content (the relayout flicker). Idempotent.
    fn close_reasoning(&mut self, events: &mut Vec<StreamEvent>) {
        if self.reasoning_started && !self.reasoning_finished {
            self.reasoning_finished = true;
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index: OPENAI_REASONING_BLOCK_INDEX,
            }));
        }
    }

    fn ingest_chunk(&mut self, chunk: ChatCompletionChunk) -> Result<Vec<StreamEvent>, ApiError> {
        let mut events = Vec::new();
        if !self.message_started {
            self.message_started = true;
            events.push(StreamEvent::MessageStart(MessageStartEvent {
                message: MessageResponse {
                    id: chunk.id.unwrap_or_default(),
                    kind: "message".to_string(),
                    role: "assistant".to_string(),
                    content: Vec::new(),
                    model: chunk.model.clone().unwrap_or_else(|| self.model.clone()),
                    stop_reason: None,
                    stop_sequence: None,
                    usage: Usage {
                        input_tokens: 0,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        output_tokens: 0,
                        output_tokens_details: None,
                    },
                    request_id: None,
                    thought_signature: None,
                    reasoning_replay: None,
                    context_management: None,
                },
            }));
        }

        if let Some(usage) = chunk.usage {
            self.usage = Some(Usage {
                input_tokens: uncached_prompt_tokens(self.config, &usage),
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: cache_read_tokens(self.config, &usage),
                output_tokens: usage.completion_tokens,
                output_tokens_details: None,
            });
        }

        for choice in chunk.choices {
            // Reasoning (`reasoning_content`) streams ahead of the answer on
            // OpenAI-compatible reasoners (DeepSeek-reasoner); dropped once the
            // answer has begun (see `push_reasoning_delta` / `close_reasoning`).
            if !self.text_started {
                if let Some(reasoning) = choice
                    .delta
                    .reasoning_content
                    .filter(|value| !value.is_empty())
                {
                    self.push_reasoning_delta(&mut events, &reasoning);
                }
            }

            if let Some(content) = choice.delta.content.filter(|value| !value.is_empty()) {
                if !self.text_started {
                    // The answer has begun: settle the reasoning block first so it
                    // reads as `done` instead of lingering open above the prose.
                    self.close_reasoning(&mut events);
                    self.text_started = true;
                    events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                        index: 0,
                        content_block: OutputContentBlock::Text {
                            text: String::new(),
                        },
                    }));
                }
                events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta { text: content },
                }));
            }

            for tool_call in choice.delta.tool_calls {
                let state = self.tool_calls.entry(tool_call.index).or_default();
                state.apply(tool_call);
                let block_index = state.block_index();
                if !state.started {
                    if let Some(start_event) = state.start_event()? {
                        state.started = true;
                        events.push(StreamEvent::ContentBlockStart(start_event));
                    } else {
                        continue;
                    }
                }
                if let Some(delta_event) = state.delta_event() {
                    events.push(StreamEvent::ContentBlockDelta(delta_event));
                }
                if choice.finish_reason.as_deref() == Some("tool_calls") && !state.stopped {
                    state.stopped = true;
                    events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                        index: block_index,
                    }));
                }
            }

            if let Some(finish_reason) = choice.finish_reason {
                self.stop_reason = Some(normalize_finish_reason(&finish_reason));
                if finish_reason == "tool_calls" {
                    for state in self.tool_calls.values_mut() {
                        if state.started && !state.stopped {
                            state.stopped = true;
                            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                                index: state.block_index(),
                            }));
                        }
                    }
                }
            }
        }

        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, ApiError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;

        let mut events = Vec::new();
        // A reasoner that produced only reasoning (no answer text / tool call yet)
        // leaves the reasoning block open — settle it at stream end.
        self.close_reasoning(&mut events);
        if self.text_started && !self.text_finished {
            self.text_finished = true;
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index: 0,
            }));
        }

        for state in self.tool_calls.values_mut() {
            if !state.started {
                if let Some(start_event) = state.start_event()? {
                    state.started = true;
                    events.push(StreamEvent::ContentBlockStart(start_event));
                    if let Some(delta_event) = state.delta_event() {
                        events.push(StreamEvent::ContentBlockDelta(delta_event));
                    }
                }
            }
            if state.started && !state.stopped {
                state.stopped = true;
                events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                    index: state.block_index(),
                }));
            }
        }

        if self.message_started {
            events.push(StreamEvent::MessageDelta(MessageDeltaEvent {
                delta: MessageDelta {
                    stop_reason: Some(
                        self.stop_reason
                            .clone()
                            .unwrap_or_else(|| "end_turn".to_string()),
                    ),
                    stop_sequence: None,
                    thought_signature: None,
                    reasoning_replay: None,
                },
                usage: self.usage.unwrap_or(Usage {
                    input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    output_tokens: 0,
                    output_tokens_details: None,
                }),
                context_management: None,
            }));
            events.push(StreamEvent::MessageStop(MessageStopEvent {}));
        }
        Ok(events)
    }
}

#[derive(Debug, Default)]
struct ToolCallState {
    openai_index: u32,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    emitted_len: usize,
    started: bool,
    stopped: bool,
}

impl ToolCallState {
    fn apply(&mut self, tool_call: DeltaToolCall) {
        self.openai_index = tool_call.index;
        if let Some(id) = tool_call.id {
            self.id = Some(id);
        }
        if let Some(name) = tool_call.function.name {
            self.name = Some(name);
        }
        if let Some(arguments) = tool_call.function.arguments {
            self.arguments.push_str(&arguments);
        }
    }

    const fn block_index(&self) -> u32 {
        self.openai_index + 1
    }

    #[allow(clippy::unnecessary_wraps)]
    fn start_event(&self) -> Result<Option<ContentBlockStartEvent>, ApiError> {
        let Some(name) = self.name.clone() else {
            return Ok(None);
        };
        let id = self
            .id
            .clone()
            .unwrap_or_else(|| format!("tool_call_{}", self.openai_index));
        Ok(Some(ContentBlockStartEvent {
            index: self.block_index(),
            content_block: OutputContentBlock::ToolUse {
                id,
                name,
                input: json!({}),
            },
        }))
    }

    fn delta_event(&mut self) -> Option<ContentBlockDeltaEvent> {
        if self.emitted_len >= self.arguments.len() {
            return None;
        }
        let delta = self.arguments[self.emitted_len..].to_string();
        self.emitted_len = self.arguments.len();
        Some(ContentBlockDeltaEvent {
            index: self.block_index(),
            delta: ContentBlockDelta::InputJsonDelta {
                partial_json: delta,
            },
        })
    }
}

/// Deserialize a field that some OpenAI-compatible gateways send as an explicit
/// `null` when empty. GLM / agentrouter emit `"tool_calls": null` (and even
/// `"choices": null`) instead of omitting the field. `#[serde(default)]` only
/// covers an *absent* field; an explicit `null` still reaches the typed
/// deserializer and aborts the whole stream with
/// `invalid type: null, expected a sequence`. Mapping `null` to the type's
/// default keeps these providers working.
fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    id: String,
    model: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    tool_calls: Vec<ResponseToolCall>,
}

#[derive(Debug, Deserialize)]
struct ResponseToolCall {
    id: String,
    function: ResponseToolFunction,
}

#[derive(Debug, Deserialize)]
struct ResponseToolFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    prompt_tokens_details: PromptTokensDetails,
}

#[derive(Debug, Default, Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: u32,
}

/// Cache-read token count to report for a provider's usage payload.
///
/// OpenAI exposes prompt-cache hits as
/// `prompt_tokens_details.cached_tokens`. Other OpenAI-compatible providers
/// may omit or repurpose that field, so unless the provider explicitly
/// advertises [`OpenAiCompatConfig::supports_cache_tokens`] we report 0
/// instead of fabricating a cache read.
const fn cache_read_tokens(config: OpenAiCompatConfig, usage: &OpenAiUsage) -> u32 {
    if config.supports_cache_tokens {
        usage.prompt_tokens_details.cached_tokens
    } else {
        0
    }
}

const fn uncached_prompt_tokens(config: OpenAiCompatConfig, usage: &OpenAiUsage) -> u32 {
    usage
        .prompt_tokens
        .saturating_sub(cache_read_tokens(config, usage))
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    /// Optional: OpenAI-compatible gateways commonly stamp `id` on the first
    /// chunk only, or omit it entirely.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
    /// Streamed chain-of-thought from OpenAI-compatible reasoners
    /// (DeepSeek-reasoner emits `reasoning_content` ahead of `content`).
    /// Surfaced as a `Thinking` block so the wait shows the model reasoning
    /// instead of a blank gap that then bursts the answer. Absent on
    /// non-reasoning providers (`None` → no behavior change).
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    tool_calls: Vec<DeltaToolCall>,
}

#[derive(Debug, Deserialize)]
struct DeltaToolCall {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: DeltaFunction,
}

#[derive(Debug, Default, Deserialize)]
struct DeltaFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// OpenAI-style `{"error": {"type", "message", "code"}}` failure envelope.
/// Shared with the ChatGPT subscription backend so both GPT paths surface the
/// human-readable message instead of dumping the raw JSON body.
#[derive(Debug, Deserialize)]
pub(crate) struct ErrorEnvelope {
    pub(crate) error: ErrorBody,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ErrorBody {
    #[serde(rename = "type")]
    pub(crate) error_type: Option<String>,
    pub(crate) message: Option<String>,
    /// Responses-API errors carry `code` (e.g. `rate_limit_exceeded`) where
    /// chat-completions errors carry `type`; fall back to it for display.
    #[serde(default)]
    pub(crate) code: Option<String>,
}

/// Whether `model` is an OpenAI-style reasoning model that takes
/// `reasoning_effort` + `max_completion_tokens` and rejects the legacy
/// `max_tokens`. A catalog fact, not a name rule: the model's row — the
/// shipped o-series and GPT-5 rows, or one a publish added — declares
/// `effort_levels`. A model no row names (gpt-4o, an Ollama or llama.cpp
/// model) keeps the legacy fields, which is the conservative side.
fn is_reasoning_model(model: &str) -> bool {
    super::declared_effort_levels(model).is_some_and(|levels| !levels.is_empty())
}

fn reasoning_effort_for(model: &str, budget_tokens: u32) -> &'static str {
    super::effort_level_for_budget(budget_tokens).gpt_for_model(model)
}

fn request_config_for_base_url(
    mut config: OpenAiCompatConfig,
    base_url: &str,
) -> OpenAiCompatConfig {
    if config.provider_name == "OpenAI" && !is_official_openai_base_url(base_url) {
        // `OPENAI_BASE_URL` can point at any OpenAI-compatible server. Keep the
        // official OpenAI-only prompt-cache extensions and cache-token accounting
        // off for those custom endpoints unless they are represented by a richer
        // first-class config. Treat them as vision-less by default too: many
        // OpenAI-compatible servers reject `image_url` content parts with a hard
        // 400, and first-class/custom configs are the opt-in path for vision.
        config.prompt_cache_strategy = PromptCacheStrategy::NoRequestControls;
        config.supports_cache_tokens = false;
        config.supports_vision = false;
    }
    config
}

fn is_official_openai_base_url(base_url: &str) -> bool {
    let trimmed = base_url.trim_end_matches('/');
    trimmed == DEFAULT_OPENAI_BASE_URL
        || trimmed == format!("{DEFAULT_OPENAI_BASE_URL}/chat/completions")
}

fn build_chat_completion_request(request: &MessageRequest, config: OpenAiCompatConfig) -> Value {
    let mut messages = Vec::new();
    if let Some(blocks) = request.system.as_ref().filter(|v| !v.is_empty()) {
        let text: String = blocks
            .iter()
            .map(|block| match block {
                crate::types::SystemBlock::Text { text, .. } => text.as_str(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            // Correct zo's hardcoded Claude identity for the served model via
            // the shared override (xAI / OpenAI-API-key / custom endpoints all
            // route here). A custom provider yields no maker → neutral wording.
            let content = crate::providers::apply_non_anthropic_identity(
                &text,
                &request.model,
                config.identity_maker(),
            );
            messages.push(json!({
                "role": "system",
                "content": content,
            }));
        }
    }
    for message in &request.messages {
        messages.extend(translate_message(message, config.supports_vision));
    }

    let mut payload = json!({
        // The catalog decides the served id. It normally answers "the same id",
        // but a provider whose selection id differs from what it serves (or a
        // model declared only in settings) is handled here rather than in a
        // per-provider special case.
        "model": crate::providers::wire_model_for_request(&request.model, request.reasoning_request()),
        "messages": messages,
        "stream": request.stream,
    });

    // Thread the extended-thinking budget through to OpenAI-style reasoning
    // models. They reject the legacy `max_tokens` (it must be
    // `max_completion_tokens`) and take an optional `reasoning_effort` —
    // without this the `/effort` budget is silently dropped for those models.
    // Every other model keeps `max_tokens` unchanged.
    //
    // Two separate decisions that used to share one `if`. The token-field
    // rename is an OpenAI-reasoning-family wire fact (those models reject
    // `max_tokens`), while carrying `reasoning_effort` is a capability an
    // operator can declare for a gateway-served reasoning model whose id Zo
    // cannot recognize. Conflating them dropped the effort field entirely for
    // every custom provider — `/effort` and the Smart band silently no-op'd.
    let is_openai_reasoning = is_reasoning_model(&request.model);
    if is_openai_reasoning {
        payload["max_completion_tokens"] = json!(request.max_tokens);
    } else {
        payload["max_tokens"] = json!(request.max_tokens);
    }
    if is_openai_reasoning || config.supports_reasoning_effort {
        // Priority: an explicit provider-neutral effort, else a legacy
        // thinking budget mapped to the model's effort tier.
        match request.reasoning_request() {
            // Same dynamic-band handling as the chatgpt_backend Responses
            // path (`resolve_effort_band` BEFORE `gpt_for_model`) — a sol
            // reached through an API-key/custom OpenAI-compatible provider
            // must not stay statically pinned while the ChatGPT-subscription
            // path goes dynamic (the two-wire-path split-brain risk).
            ReasoningRequest::Effort(level) => {
                let level = match request.effort_band_ceiling {
                    Some(ceiling) => super::resolve_effort_band(
                        level,
                        ceiling,
                        &request.model,
                        super::band_difficulty_for_request(request),
                    ),
                    None => level,
                };
                payload["reasoning_effort"] = json!(level.gpt_for_model(&request.model));
            }
            ReasoningRequest::BudgetTokens(budget) => {
                payload["reasoning_effort"] = json!(reasoning_effort_for(&request.model, budget));
            }
            ReasoningRequest::Auto => {}
        }
    }

    if request.stream && should_request_stream_usage(config) {
        payload["stream_options"] = json!({ "include_usage": true });
    }

    apply_prompt_cache_controls(&mut payload, request, config);

    if let Some(tools) = &request.tools {
        payload["tools"] =
            Value::Array(tools.iter().map(openai_tool_definition).collect::<Vec<_>>());
    }
    if let Some(tool_choice) = &request.tool_choice {
        payload["tool_choice"] = openai_tool_choice(tool_choice);
    }

    payload
}

fn apply_prompt_cache_controls(
    payload: &mut Value,
    request: &MessageRequest,
    config: OpenAiCompatConfig,
) {
    if !config.prompt_cache_strategy.sends_openai_prompt_cache_key() {
        return;
    }
    let key = prompt_cache_key(&request.model, request, "");
    payload["prompt_cache_key"] = json!(key);
    if let Some(retention) = config
        .prompt_cache_strategy
        .prompt_cache_retention(&request.model)
    {
        payload["prompt_cache_retention"] = json!(retention);
    }
}

fn translate_message(message: &InputMessage, supports_vision: bool) -> Vec<Value> {
    if message.role == "assistant" {
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        for block in &message.content {
            match block {
                InputContentBlock::Text { text: value, .. } => text.push_str(value),
                InputContentBlock::ToolUse { .. } => {
                    let Some(ToolLedgerView::ToolUse { id, name, input }) =
                        ToolLedgerView::from_input_block(block)
                    else {
                        unreachable!("tool use block must project to tool use ledger view");
                    };
                    tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": input.to_string(),
                        }
                    }));
                }
                // Anthropic reasoning blocks are provider-opaque and never
                // lowered into an OpenAI-compatible request.
                InputContentBlock::ToolResult { .. }
                | InputContentBlock::Image { .. }
                | InputContentBlock::Document { .. }
                | InputContentBlock::Thinking { .. }
                | InputContentBlock::RedactedThinking { .. } => {}
            }
        }
        if text.is_empty() && tool_calls.is_empty() {
            Vec::new()
        } else {
            let mut message = json!({
                "role": "assistant",
                "content": (!text.is_empty()).then_some(text),
            });
            // OpenAI-compatible servers (e.g. DeepSeek) reject an empty
            // `tool_calls` array; the field must be omitted when there are no
            // tool calls rather than serialized as `[]`.
            if !tool_calls.is_empty() {
                message["tool_calls"] = Value::Array(tool_calls);
            }
            vec![message]
        }
    } else {
        let mut translated = Vec::new();
        let mut pending_user_blocks = Vec::new();
        for block in &message.content {
            match block {
                InputContentBlock::Text { .. } | InputContentBlock::Image { .. } => {
                    pending_user_blocks.push(block);
                }
                InputContentBlock::ToolResult { .. } => {
                    let Some(ToolLedgerView::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    }) = ToolLedgerView::from_input_block(block)
                    else {
                        unreachable!("tool result block must project to tool result ledger view");
                    };
                    flush_openai_user_message(&mut translated, &mut pending_user_blocks, supports_vision);
                    translated.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": super::flatten_tool_result_content(content),
                    }));
                }
                InputContentBlock::ToolUse { .. }
                | InputContentBlock::Document { .. }
                | InputContentBlock::Thinking { .. }
                | InputContentBlock::RedactedThinking { .. } => {}
            }
        }
        flush_openai_user_message(&mut translated, &mut pending_user_blocks, supports_vision);
        translated
    }
}

fn flush_openai_user_message(
    messages: &mut Vec<Value>,
    blocks: &mut Vec<&InputContentBlock>,
    supports_vision: bool,
) {
    if blocks.is_empty() {
        return;
    }

    let has_image = supports_vision
        && blocks
            .iter()
            .any(|block| matches!(block, InputContentBlock::Image { .. }));

    if has_image {
        let content = blocks
            .iter()
            .filter_map(|block| match block {
                InputContentBlock::Text { text, .. } => Some(json!({
                    "type": "text",
                    "text": text,
                })),
                InputContentBlock::Image { source, .. } => Some(openai_image_content(source)),
                InputContentBlock::Document { .. }
                | InputContentBlock::ToolUse { .. }
                | InputContentBlock::ToolResult { .. }
                | InputContentBlock::Thinking { .. }
                | InputContentBlock::RedactedThinking { .. } => None,
            })
            .collect::<Vec<_>>();
        if !content.is_empty() {
            messages.push(json!({
                "role": "user",
                "content": content,
            }));
        }
    } else {
        // Either the provider is text-only, or it is vision-less and images were
        // present. Emit each text block as its own user message (unchanged), and
        // degrade any image to a text placeholder so the model still learns an
        // image was supplied instead of the server hard-400ing on `image_url`.
        for block in blocks.iter() {
            match block {
                InputContentBlock::Text { text, .. } => {
                    messages.push(json!({
                        "role": "user",
                        "content": text,
                    }));
                }
                InputContentBlock::Image { source, .. } => {
                    messages.push(json!({
                        "role": "user",
                        "content": image_placeholder_text(source),
                    }));
                }
                InputContentBlock::Document { .. }
                | InputContentBlock::ToolUse { .. }
                | InputContentBlock::ToolResult { .. }
                | InputContentBlock::Thinking { .. }
                | InputContentBlock::RedactedThinking { .. } => {}
            }
        }
    }

    blocks.clear();
}

/// Text stand-in for an image sent to a vision-less OpenAI-compatible endpoint.
/// Delegates to the shared [`crate::providers::image_omitted_placeholder`].
fn image_placeholder_text(source: &ImageSource) -> String {
    super::image_omitted_placeholder(&source.media_type)
}

fn openai_image_content(source: &ImageSource) -> Value {
    json!({
        "type": "image_url",
        "image_url": {
            "url": super::image_data_url(source),
        },
    })
}

fn openai_tool_definition(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

fn openai_tool_choice(tool_choice: &ToolChoice) -> Value {
    match tool_choice {
        ToolChoice::Auto => Value::String("auto".to_string()),
        ToolChoice::Any => Value::String("required".to_string()),
        ToolChoice::None => Value::String("none".to_string()),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "function": { "name": name },
        }),
    }
}

fn should_request_stream_usage(config: OpenAiCompatConfig) -> bool {
    config.request_stream_usage
}

fn normalize_response(
    model: &str,
    response: ChatCompletionResponse,
    config: OpenAiCompatConfig,
) -> Result<MessageResponse, ApiError> {
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or(ApiError::InvalidSseFrame(
            "chat completion response missing choices",
        ))?;
    let mut content = Vec::new();
    if let Some(text) = choice.message.content.filter(|value| !value.is_empty()) {
        content.push(OutputContentBlock::Text { text });
    }
    for tool_call in choice.message.tool_calls {
        content.push(OutputContentBlock::ToolUse {
            id: tool_call.id,
            name: tool_call.function.name,
            input: parse_tool_arguments(&tool_call.function.arguments),
        });
    }

    Ok(MessageResponse {
        id: response.id,
        kind: "message".to_string(),
        role: choice.message.role,
        content,
        model: response.model.if_empty_then(model.to_string()),
        stop_reason: choice
            .finish_reason
            .map(|value| normalize_finish_reason(&value)),
        stop_sequence: None,
        usage: Usage {
            input_tokens: response
                .usage
                .as_ref()
                .map_or(0, |usage| uncached_prompt_tokens(config, usage)),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: response
                .usage
                .as_ref()
                .map_or(0, |usage| cache_read_tokens(config, usage)),
            output_tokens: response
                .usage
                .as_ref()
                .map_or(0, |usage| usage.completion_tokens),
            output_tokens_details: None,
        },
        request_id: None,
        thought_signature: None,
        reasoning_replay: None,
        context_management: None,
    })
}

fn parse_tool_arguments(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({ "raw": arguments }))
}

fn next_sse_frame(buffer: &mut Vec<u8>, scanned: &mut usize) -> Option<String> {
    // Keep a small overlap so separators split across the previous resume point
    // are still found. The longest supported separator is "\r\n\r\n".
    let start = (*scanned).saturating_sub(3);
    let separator = buffer[start..]
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| (start + position, 2))
        .or_else(|| {
            buffer[start..]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| (start + position, 4))
        });

    let Some((position, separator_len)) = separator else {
        *scanned = buffer.len().saturating_sub(3);
        return None;
    };

    let frame = String::from_utf8_lossy(&buffer[..position]).into_owned();
    buffer.drain(..position + separator_len);
    *scanned = 0;
    Some(frame)
}

fn parse_sse_frame(frame: &str) -> Result<Option<ChatCompletionChunk>, ApiError> {
    let trimmed = frame.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut payload = String::new();
    for line in trimmed.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            if !payload.is_empty() {
                payload.push('\n');
            }
            payload.push_str(data.trim_start());
        }
    }
    if payload.is_empty() {
        return Ok(None);
    }
    if payload == "[DONE]" {
        return Ok(None);
    }
    if let Some(error) = parse_stream_error_frame(&payload) {
        return Err(error);
    }
    serde_json::from_str(&payload)
        .map(Some)
        .map_err(ApiError::from)
}

/// A mid-stream `{"error": {...}}` frame, which OpenAI-compatible gateways send
/// in place of a chunk when the turn fails after the response headers are
/// already out. Every chunk field is optional, so without this the envelope
/// would deserialize as an empty chunk and the failure would be dropped.
///
/// The substring prefilter keeps ordinary chunks off the second `Value` parse;
/// a text delta that merely contains the word is rejected by the shape check.
fn parse_stream_error_frame(payload: &str) -> Option<ApiError> {
    if !payload.contains("\"error\"") {
        return None;
    }
    let value = serde_json::from_str::<Value>(payload).ok()?;
    value.get("error")?;
    let envelope = serde_json::from_value::<ErrorEnvelope>(value).ok()?;
    let error_type = envelope.error.error_type.or(envelope.error.code);
    let retryable = [error_type.as_deref(), envelope.error.message.as_deref()]
        .into_iter()
        .flatten()
        .any(|part| core_types::retry_signal::is_capacity_text(&part.to_ascii_lowercase()));
    Some(ApiError::StreamApi {
        error_type,
        message: envelope.error.message,
        body: payload.to_string(),
        retryable,
    })
}

#[must_use]
pub fn has_api_key(key: &str) -> bool {
    read_api_key(key).ok().flatten().is_some()
}

/// An API key named `key`: the environment ladder first, then zo's saved
/// key — the one order every OpenAI-compatible client and the credentials
/// that stand beside one (the Grok CLI's, for xAI) read it in.
pub(crate) fn read_api_key(key: &str) -> Result<Option<String>, ApiError> {
    Ok(read_env_non_empty(key)?.or_else(|| read_saved_api_key_non_empty(key)))
}

fn read_saved_api_key_non_empty(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    crate::oauth_store::load_openai_compat_api_key(key)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Whether a *custom* base URL is configured for `config` (its `base_url_env`
/// is set to a non-empty value). A custom endpoint is treated as a self-hosted
/// / proxy target that may legitimately need no API key, unlike the official
/// cloud endpoint reached via the default base URL.
#[must_use]
pub fn has_custom_base_url(config: OpenAiCompatConfig) -> bool {
    read_env_non_empty(config.base_url_env)
        .ok()
        .flatten()
        .is_some()
}

#[must_use]
pub fn read_base_url(config: OpenAiCompatConfig) -> String {
    std::env::var(config.base_url_env).unwrap_or_else(|_| config.default_base_url.to_string())
}

#[derive(Debug, Clone, Deserialize)]
struct ModelsListResponse {
    #[serde(default)]
    data: Vec<ModelListEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelListEntry {
    id: String,
}

/// List the model ids a running local OpenAI-compatible server advertises at
/// `GET {base_url}/models`. Both Ollama (via its OpenAI-compat shim) and LM
/// Studio answer with `{ "data": [ { "id": ... } ] }`. A short timeout keeps a
/// not-running server from stalling the caller, and any failure (server down,
/// non-2xx, bad JSON) yields an empty list rather than an error so callers
/// degrade cleanly to "nothing discovered".
pub async fn discover_models(base_url: &str) -> Vec<String> {
    let endpoint = format!("{}/models", base_url.trim_end_matches('/'));
    let Ok(response) = super::shared_http_client()
        .get(&endpoint)
        .timeout(Duration::from_secs(2))
        .send()
        .await
    else {
        return Vec::new();
    };
    if !response.status().is_success() {
        return Vec::new();
    }
    match response.json::<ModelsListResponse>().await {
        Ok(body) => body.data.into_iter().map(|entry| entry.id).collect(),
        Err(_) => Vec::new(),
    }
}

/// Authenticated variant of [`discover_models`] for cloud OpenAI-compatible
/// providers. Unlike the local-server helper, errors are returned so `/connect`
/// can distinguish "key missing/invalid" from a successful connection.
pub async fn discover_models_with_bearer(
    base_url: &str,
    bearer: &str,
) -> Result<Vec<String>, ApiError> {
    let endpoint = format!("{}/models", base_url.trim_end_matches('/'));
    let response = super::shared_http_client()
        .get(&endpoint)
        .bearer_auth(bearer.trim())
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    let response = expect_success(response).await?;
    let body = response.json::<ModelsListResponse>().await?;
    Ok(body.data.into_iter().map(|entry| entry.id).collect())
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn request_id_from_headers(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(REQUEST_ID_HEADER)
        .or_else(|| headers.get(ALT_REQUEST_ID_HEADER))
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
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
        .map(std::time::Duration::from_secs);

    let body = response.text().await.unwrap_or_default();
    let parsed_error = serde_json::from_str::<ErrorEnvelope>(&body).ok();
    let retryable = is_retryable_status(status);

    Err(ApiError::Api {
        status,
        error_type: parsed_error
            .as_ref()
            .and_then(|error| error.error.error_type.clone()),
        message: parsed_error
            .as_ref()
            .and_then(|error| error.error.message.clone()),
        body,
        retryable,
        retry_after,
    })
}

const fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    // Match the Anthropic/OpenAI SDK `shouldRetry`: 408/409/429 plus every
    // server error (>= 500), so a 529 `overloaded_error` (and other 5xx the old
    // fixed whitelist omitted) is retried transparently instead of surfacing.
    let code = status.as_u16();
    matches!(code, 408 | 409 | 429) || code >= 500
}

fn normalize_finish_reason(value: &str) -> String {
    match value {
        "stop" => "end_turn",
        "tool_calls" => "tool_use",
        other => other,
    }
    .to_string()
}

trait StringExt {
    fn if_empty_then(self, fallback: String) -> String;
}

impl StringExt for String {
    fn if_empty_then(self, fallback: String) -> String {
        if self.is_empty() { fallback } else { self }
    }
}

#[cfg(test)]
mod tests;
