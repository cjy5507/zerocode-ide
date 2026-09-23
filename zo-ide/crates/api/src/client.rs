use crate::credential::CredentialMiss;
use crate::error::ApiError;
use crate::prompt_cache::{PromptCache, PromptCacheRecord, PromptCacheStats};
use crate::providers::anthropic::{self, AnthropicClient, AnthropicRetryNotice, AuthSource};
use crate::providers::chatgpt_backend::{self, ChatGptBackendClient};
use crate::providers::gemini_code_assist::{self, GeminiCodeAssistClient};
use crate::providers::openai_compat::{self, OpenAiCompatClient, OpenAiCompatConfig};
use crate::providers::{self, NON_CLAUDE_ADAPTERS_ENV, ProviderKind};
use crate::sync_bridge::run_blocking;
use crate::types::{MessageRequest, MessageResponse, StreamEvent};
use core_types::OpenAiOAuthTokens;
use serde::{Deserialize, Serialize};

/// Credential mechanism selected for one model-catalog row.
///
/// `Auto` preserves each provider's existing credential precedence. The other
/// variants are strict: construction either uses that mechanism or fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthRoute {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "oauth")]
    OAuth,
    #[serde(rename = "api-key")]
    ApiKey,
}

impl AuthRoute {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OAuth => "oauth",
            Self::ApiKey => "api-key",
        }
    }
}

fn provider_name(provider_kind: ProviderKind) -> &'static str {
    match provider_kind {
        ProviderKind::Anthropic => "Anthropic",
        ProviderKind::Xai => "xAI",
        ProviderKind::OpenAi => "OpenAI",
        ProviderKind::Google => "Google",
        ProviderKind::Ollama => "Ollama",
    }
}

fn anthropic_auth_for_route(
    auth_route: AuthRoute,
    auth: Option<AuthSource>,
) -> Result<AuthSource, ApiError> {
    let auth = match (auth_route, auth) {
        (_, Some(auth)) => auth,
        (AuthRoute::Auto, None) => AuthSource::from_env()?,
        (AuthRoute::OAuth, None) => AuthSource::from_oauth_only()?,
        (AuthRoute::ApiKey, None) => AuthSource::from_api_key_only()?,
    };
    match auth_route {
        AuthRoute::Auto => Ok(auth),
        AuthRoute::OAuth => auth
            .bearer_token()
            .map(|token| AuthSource::BearerToken(token.to_string()))
            .ok_or_else(|| {
                ApiError::missing_auth_route_credentials("Anthropic", auth_route.as_str())
            }),
        AuthRoute::ApiKey => auth
            .api_key()
            .map(|key| AuthSource::ApiKey(key.to_string()))
            .ok_or_else(|| {
                ApiError::missing_auth_route_credentials("Anthropic", auth_route.as_str())
            }),
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ProviderClient {
    Anthropic(AnthropicClient),
    Xai(OpenAiCompatClient),
    OpenAi(OpenAiCompatClient),
    Google(OpenAiCompatClient),
    /// Gemini CLI-compatible Code Assist backend over Google OAuth.
    GeminiCodeAssist(GeminiCodeAssistClient),
    Ollama(OpenAiCompatClient),
    /// ChatGPT subscription backend (Responses API over an OAuth access token).
    ChatGpt(ChatGptBackendClient),
}

impl ProviderClient {
    pub fn from_model(model: &str) -> Result<Self, ApiError> {
        Self::from_model_with_auth_route(model, AuthRoute::Auto)
    }

    /// Pin the ChatGPT backend's prompt-cache scope to a host-stable id (the
    /// zo session id) so provider cache keys survive client rebuilds — model
    /// swaps and OAuth rotations reconstruct the client, and its default
    /// per-instance scope would roll the key each time (see
    /// `ChatGptBackendClient::with_cache_scope`). No-op for every other
    /// provider: Anthropic caching is prefix-based (no key), and the
    /// OpenAI-compatible path derives its key without a session component.
    #[must_use]
    pub fn with_cache_scope(self, scope: &str) -> Self {
        match self {
            Self::ChatGpt(client) => Self::ChatGpt(client.with_cache_scope(scope)),
            other => other,
        }
    }

    /// The pinned ChatGPT cache scope, if this client carries one — read at
    /// 401-recovery rebuild time to carry the scope onto the fresh client.
    #[must_use]
    pub fn pinned_cache_scope(&self) -> Option<&str> {
        match self {
            Self::ChatGpt(client) => client.pinned_cache_scope(),
            _ => None,
        }
    }

    pub fn from_model_with_anthropic_auth(
        model: &str,
        anthropic_auth: Option<AuthSource>,
    ) -> Result<Self, ApiError> {
        Self::from_model_with_auth_route_and_anthropic_auth(
            model,
            AuthRoute::Auto,
            anthropic_auth,
        )
    }

    pub fn from_model_with_auth_route(
        model: &str,
        auth_route: AuthRoute,
    ) -> Result<Self, ApiError> {
        Self::from_model_with_auth_route_and_anthropic_auth(model, auth_route, None)
    }

    pub fn from_model_with_auth_route_and_anthropic_auth(
        model: &str,
        auth_route: AuthRoute,
        anthropic_auth: Option<AuthSource>,
    ) -> Result<Self, ApiError> {
        let resolved_model = providers::resolve_model_alias(model);

        // A user-defined provider (ZO_CUSTOM_PROVIDERS) takes over once the
        // static registry has missed. It reuses the OpenAI-compatible client,
        // wrapped in the existing `OpenAi` variant so no new `ProviderClient`
        // arm or exhaustive-match edit is needed. Defining the provider is its
        // own opt-in, so this MUST sit ahead of the built-in non-Claude adapter
        // gate: otherwise a custom model named `grok-*` is misclassified as the
        // first-party xAI adapter and rejected before the custom route can run.
        if let Some(custom) = providers::custom_provider_for_model(&resolved_model) {
            return match auth_route {
                AuthRoute::Auto | AuthRoute::ApiKey => {
                    Ok(Self::OpenAi(build_custom_client(&custom)?))
                }
                AuthRoute::OAuth => Err(ApiError::unsupported_auth_route(
                    custom.config.provider_name,
                    auth_route.as_str(),
                )),
            };
        }

        if auth_route == AuthRoute::Auto {
            if let Some(provider_kind) =
                providers::explicit_non_claude_provider_kind(&resolved_model)
            {
                if !providers::provider_enabled(provider_kind) {
                    return Err(ApiError::unsupported_provider(
                        provider_name(provider_kind),
                        NON_CLAUDE_ADAPTERS_ENV,
                    ));
                }
            }
        }

        Self::from_provider_kind_with_auth_route_and_anthropic_auth(
            providers::detect_provider_kind(&resolved_model),
            auth_route,
            anthropic_auth,
        )
    }

    pub fn from_provider_kind_with_auth_route(
        provider_kind: ProviderKind,
        auth_route: AuthRoute,
    ) -> Result<Self, ApiError> {
        Self::from_provider_kind_with_auth_route_and_anthropic_auth(
            provider_kind,
            auth_route,
            None,
        )
    }

    pub fn from_provider_kind_with_auth_route_and_anthropic_auth(
        provider_kind: ProviderKind,
        auth_route: AuthRoute,
        anthropic_auth: Option<AuthSource>,
    ) -> Result<Self, ApiError> {
        if auth_route == AuthRoute::Auto && !providers::provider_enabled(provider_kind) {
            return Err(ApiError::unsupported_provider(
                provider_name(provider_kind),
                NON_CLAUDE_ADAPTERS_ENV,
            ));
        }

        match provider_kind {
            ProviderKind::Anthropic => Ok(Self::Anthropic(AnthropicClient::from_auth(
                anthropic_auth_for_route(auth_route, anthropic_auth)?,
            ))),
            ProviderKind::Xai => match auth_route {
                AuthRoute::Auto => Ok(Self::Xai(xai_client()?)),
                AuthRoute::ApiKey => Ok(Self::Xai(openai_compat_api_key_for_route(
                    OpenAiCompatConfig::xai(),
                )?)),
                AuthRoute::OAuth => Ok(Self::Xai(xai_grok_login_client()?)),
            },
            ProviderKind::OpenAi => match auth_route {
                AuthRoute::Auto => Ok(match load_fresh_openai_oauth() {
                    Some(tokens) => Self::ChatGpt(ChatGptBackendClient::new(
                        tokens.access_token,
                        tokens.account_id,
                    )),
                    None => Self::OpenAi(openai_compat_from_env(OpenAiCompatConfig::openai())?),
                }),
                AuthRoute::OAuth => load_fresh_openai_oauth().map_or_else(
                    || {
                        Err(ApiError::missing_auth_route_credentials(
                            "OpenAI",
                            auth_route.as_str(),
                        ))
                    },
                    |tokens| {
                        Ok(Self::ChatGpt(ChatGptBackendClient::new(
                            tokens.access_token,
                            tokens.account_id,
                        )))
                    },
                ),
                AuthRoute::ApiKey => Ok(Self::OpenAi(openai_compat_api_key_for_route(
                    OpenAiCompatConfig::openai(),
                )?)),
            },
            ProviderKind::Google => match auth_route {
                AuthRoute::Auto => Ok(
                    if let Some(tokens) = gemini_code_assist::load_fresh_oauth() {
                        Self::GeminiCodeAssist(GeminiCodeAssistClient::from_oauth(&tokens))
                    } else {
                        Self::Google(google_openai_compat_from_env()?)
                    },
                ),
                AuthRoute::OAuth => gemini_code_assist::load_fresh_oauth().map_or_else(
                    || {
                        Err(ApiError::missing_auth_route_credentials(
                            "Google",
                            auth_route.as_str(),
                        ))
                    },
                    |tokens| {
                        Ok(Self::GeminiCodeAssist(GeminiCodeAssistClient::from_oauth(
                            &tokens,
                        )))
                    },
                ),
                AuthRoute::ApiKey => Ok(Self::Google(openai_compat_api_key_for_route(
                    OpenAiCompatConfig::google(),
                )?)),
            },
            ProviderKind::Ollama => match auth_route {
                AuthRoute::Auto => Ok(Self::Ollama(
                    OpenAiCompatClient::from_env(OpenAiCompatConfig::ollama()).or_else(|_| {
                        OpenAiCompatClient::from_env_optional_auth(OpenAiCompatConfig::ollama())
                    })?,
                )),
                AuthRoute::OAuth | AuthRoute::ApiKey => Err(ApiError::unsupported_auth_route(
                    "Ollama",
                    auth_route.as_str(),
                )),
            },
        }
    }

    /// Test/diagnostic seam: a ChatGPT-backend (Codex Responses) client aimed
    /// at `base_url` with `access_token` supplied verbatim, bypassing every
    /// credential resolver.
    ///
    /// This exists so the *live request path* can be exercised against a local
    /// endpoint that speaks the real Responses SSE contract. The routing
    /// probe's wire call failed 100% of the time for weeks — first because a
    /// `stream: false` body is rejected outright, then because the terminal
    /// frame carries `"output": []` under `store: false` — and neither failure
    /// is reachable from a parse-level test. Credential resolution is the only
    /// reason that path could not be tested, so this constructor removes
    /// exactly that obstacle and nothing else: no wire behavior differs from a
    /// normally-built [`Self::ChatGpt`] client.
    #[must_use]
    pub fn chatgpt_backend_at(base_url: &str, access_token: &str) -> Self {
        Self::ChatGpt(ChatGptBackendClient::new(access_token, None).with_base_url(base_url))
    }

    #[must_use]
    pub const fn provider_kind(&self) -> ProviderKind {
        match self {
            Self::Anthropic(_) => ProviderKind::Anthropic,
            Self::Xai(_) => ProviderKind::Xai,
            Self::OpenAi(_) | Self::ChatGpt(_) => ProviderKind::OpenAi,
            Self::Google(_) | Self::GeminiCodeAssist(_) => ProviderKind::Google,
            Self::Ollama(_) => ProviderKind::Ollama,
        }
    }

    #[must_use]
    pub fn with_prompt_cache(self, prompt_cache: PromptCache) -> Self {
        match self {
            Self::Anthropic(client) => Self::Anthropic(client.with_prompt_cache(prompt_cache)),
            other => other,
        }
    }

    /// Return a copy with the Anthropic auth swapped (no-op for other
    /// providers). Used to retry a 401'd streaming request with a freshly
    /// refreshed OAuth bearer without rebuilding the whole client.
    #[must_use]
    pub fn with_anthropic_auth(self, auth: AuthSource) -> Self {
        match self {
            Self::Anthropic(client) => Self::Anthropic(client.with_auth(auth)),
            other => other,
        }
    }

    /// Return a copy with a foreground retry notice callback installed on the
    /// Anthropic client. No-op for non-Anthropic providers.
    #[must_use]
    pub fn with_anthropic_retry_notice_callback(
        self,
        callback: impl Fn(AnthropicRetryNotice) + Send + Sync + 'static,
    ) -> Self {
        match self {
            Self::Anthropic(client) => Self::Anthropic(client.with_retry_notice_callback(callback)),
            other => other,
        }
    }

    /// Return a copy whose Anthropic client surfaces a rate limit immediately.
    /// No-op for non-Anthropic providers.
    #[must_use]
    pub fn with_rate_limit_fail_fast(self) -> Self {
        match self {
            Self::Anthropic(client) => Self::Anthropic(client.with_rate_limit_fail_fast()),
            other => other,
        }
    }

    #[must_use]
    pub fn prompt_cache_stats(&self) -> Option<PromptCacheStats> {
        match self {
            Self::Anthropic(client) => client.prompt_cache_stats(),
            Self::Xai(_)
            | Self::OpenAi(_)
            | Self::Google(_)
            | Self::GeminiCodeAssist(_)
            | Self::Ollama(_)
            | Self::ChatGpt(_) => None,
        }
    }

    #[must_use]
    pub fn take_last_prompt_cache_record(&self) -> Option<PromptCacheRecord> {
        match self {
            Self::Anthropic(client) => client.take_last_prompt_cache_record(),
            Self::Xai(_)
            | Self::OpenAi(_)
            | Self::Google(_)
            | Self::GeminiCodeAssist(_)
            | Self::Ollama(_)
            | Self::ChatGpt(_) => None,
        }
    }

    /// Whether this long-lived OAuth-backed provider client should be rebuilt
    /// before the next request because its saved bearer is expired or within the
    /// provider's refresh skew. This is intentionally a cheap local token-store
    /// check; rebuilding runs provider loaders that may perform network OAuth /
    /// account setup and must not happen on every turn.
    #[must_use]
    pub fn oauth_rebuild_needed(&self) -> bool {
        use crate::managed_account::{ManagedProvider, reload_pending};

        match self {
            // A switch the IDE pushed rebuilds even a token that has not
            // expired: the old bearer is still perfectly valid, it just
            // belongs to the account the person just left.
            Self::GeminiCodeAssist(_) => {
                reload_pending(ManagedProvider::Google)
                    || crate::oauth_store::load_google_code_assist_oauth()
                        .ok()
                        .flatten()
                        .is_some_and(|tokens| gemini_code_assist::token_expired(&tokens))
            }
            Self::ChatGpt(_) => {
                reload_pending(ManagedProvider::OpenAi)
                    || crate::oauth_store::load_openai_oauth()
                        .ok()
                        .flatten()
                        .is_some_and(|tokens| openai_oauth_expired(&tokens))
            }
            Self::Anthropic(_) => {
                reload_pending(ManagedProvider::Anthropic)
                    || crate::providers::anthropic::managed_claude_auth_changed()
            }
            Self::Xai(_)
            | Self::OpenAi(_)
            | Self::Google(_)
            | Self::Ollama(_) => false,
        }
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        match self {
            Self::Anthropic(client) => client.send_message(request).await,
            Self::Xai(client)
            | Self::OpenAi(client)
            | Self::Google(client)
            | Self::Ollama(client) => client.send_message(request).await,
            Self::GeminiCodeAssist(client) => client.send_message(request).await,
            Self::ChatGpt(client) => client.send_message(request).await,
        }
    }

    pub async fn generate_image(
        &self,
        model: &str,
        prompt: &str,
        size: Option<&str>,
        quality: Option<&str>,
    ) -> Result<String, ApiError> {
        match self {
            Self::ChatGpt(client) => client.generate_image(model, prompt, size, quality).await,
            _ => Err(ApiError::unsupported_auth_route(
                "OpenAI image generation",
                "non-OAuth",
            )),
        }
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        match self {
            Self::Anthropic(client) => client
                .stream_message(request)
                .await
                .map(|stream| MessageStream::Anthropic(Box::new(stream))),
            Self::Xai(client)
            | Self::OpenAi(client)
            | Self::Google(client)
            | Self::Ollama(client) => client
                .stream_message(request)
                .await
                .map(|stream| MessageStream::OpenAiCompat(Box::new(stream))),
            Self::GeminiCodeAssist(client) => client
                .stream_message(request)
                .await
                .map(|stream| MessageStream::GeminiCodeAssist(Box::new(stream))),
            Self::ChatGpt(client) => client
                .stream_message(request)
                .await
                .map(|stream| MessageStream::ChatGpt(Box::new(stream))),
        }
    }
}

#[derive(Debug)]
pub enum MessageStream {
    // All variants boxed: each provider stream embeds a live response plus
    // retry state (the ChatGpt one carries a whole client + request), so
    // boxing keeps `MessageStream` itself pointer-sized per arm. Match arms
    // read through the box transparently via `Deref`.
    Anthropic(Box<anthropic::MessageStream>),
    OpenAiCompat(Box<openai_compat::MessageStream>),
    GeminiCodeAssist(Box<gemini_code_assist::GeminiCodeAssistStream>),
    ChatGpt(Box<chatgpt_backend::ChatGptStream>),
}

impl MessageStream {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Anthropic(stream) => stream.request_id(),
            Self::OpenAiCompat(stream) => stream.request_id(),
            Self::GeminiCodeAssist(stream) => stream.request_id(),
            Self::ChatGpt(_) => None,
        }
    }

    /// Unified rate-limit snapshot from the response headers, if the provider
    /// surfaced one. Only Anthropic (subscription / OAuth) carries the unified
    /// `anthropic-ratelimit-unified-*` headers; OpenAI-compatible providers
    /// return `None`.
    #[must_use]
    pub fn rate_limit(&self) -> Option<core_types::RateLimitSnapshot> {
        match self {
            Self::Anthropic(stream) => stream.rate_limit(),
            Self::OpenAiCompat(_) | Self::GeminiCodeAssist(_) | Self::ChatGpt(_) => None,
        }
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        match self {
            Self::Anthropic(stream) => stream.next_event().await,
            Self::OpenAiCompat(stream) => stream.next_event().await,
            Self::GeminiCodeAssist(stream) => stream.next_event().await,
            Self::ChatGpt(stream) => stream.next_event().await,
        }
    }

    /// Install a mid-stream retry sink. The `ChatGpt`, `OpenAiCompat`, and
    /// `GeminiCodeAssist` backends perform internal transparent pre-commit
    /// restarts that the establish-time retry layer never sees, so without
    /// this the reconnect pause reads as a freeze. `Anthropic` deliberately
    /// stays silent (see its `restart` doc: transparent recovery must not
    /// write into the TUI).
    #[must_use]
    pub fn with_stream_retry_notice(
        self,
        callback: impl Fn(core_types::StreamRetryNotice) + Send + Sync + 'static,
    ) -> Self {
        match self {
            Self::ChatGpt(stream) => {
                Self::ChatGpt(Box::new(stream.with_retry_notice_callback(callback)))
            }
            Self::OpenAiCompat(stream) => {
                Self::OpenAiCompat(Box::new(stream.with_retry_notice_callback(callback)))
            }
            Self::GeminiCodeAssist(stream) => {
                Self::GeminiCodeAssist(Box::new(stream.with_retry_notice_callback(callback)))
            }
            Self::Anthropic(_) => self,
        }
    }
}

pub use anthropic::{
    OAuthTokenSet, oauth_token_is_expired, resolve_saved_oauth_token, resolve_startup_auth_source,
};
#[must_use]
pub fn read_base_url() -> String {
    anthropic::read_base_url()
}

#[must_use]
pub fn read_xai_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::xai())
}

/// Build an OpenAI-compatible client from the environment. When the provider
/// points at a *custom* base URL (a self-hosted or proxy endpoint), fall back
/// to optional auth like Ollama so a keyless endpoint works; the official
/// cloud endpoints keep requiring a key, surfacing a clean missing-credentials
/// error instead of a downstream 401.
fn openai_compat_from_env(config: OpenAiCompatConfig) -> Result<OpenAiCompatClient, ApiError> {
    if openai_compat::has_custom_base_url(config) {
        OpenAiCompatClient::from_env(config)
            .or_else(|_| OpenAiCompatClient::from_env_optional_auth(config))
    } else {
        OpenAiCompatClient::from_env(config)
    }
}

/// The xAI client for the automatic route. A custom `XAI_BASE_URL` keeps the
/// optional-auth path a self-hosted endpoint needs; the official endpoint
/// takes whatever the one xAI resolution finds — the API key, else the Grok
/// CLI's login (t-6248) — the same answer the model list gets.
fn xai_client() -> Result<OpenAiCompatClient, ApiError> {
    let config = OpenAiCompatConfig::xai();
    if openai_compat::has_custom_base_url(config) {
        return openai_compat_from_env(config);
    }
    providers::cli_sessions::resolve_xai_credential()
        .map(|credential| OpenAiCompatClient::new(credential.bearer, config))
        .map_err(|miss| match miss {
            CredentialMiss::Absent => {
                ApiError::missing_credentials(config.provider_name, config.credential_env_vars())
            }
            CredentialMiss::Unusable(why) => ApiError::Auth(why),
        })
}

/// The xAI client for the OAuth route: the Grok CLI's own login, the rung
/// of the same resolution that holds it.
fn xai_grok_login_client() -> Result<OpenAiCompatClient, ApiError> {
    let config = OpenAiCompatConfig::xai();
    providers::cli_sessions::grok_session()
        .map(|session| OpenAiCompatClient::new(session.bearer, config))
        .map_err(|miss| match miss {
            CredentialMiss::Absent => {
                ApiError::missing_auth_route_credentials(config.provider_name, AuthRoute::OAuth.as_str())
            }
            CredentialMiss::Unusable(why) => ApiError::Auth(why),
        })
}

fn openai_compat_api_key_for_route(
    config: OpenAiCompatConfig,
) -> Result<OpenAiCompatClient, ApiError> {
    OpenAiCompatClient::from_env(config).map_err(|error| match error {
        ApiError::MissingCredentials { .. } => {
            ApiError::missing_auth_route_credentials(config.provider_name, "api-key")
        }
        error => error,
    })
}

/// Build the Gemini/OpenAI-compatible fallback client. Custom `GOOGLE_BASE_URL`
/// endpoints keep the old optional-auth behavior so keyless proxies remain
/// supported. On the official Google endpoint, OAuth/ADC wins over
/// `GOOGLE_API_KEY` to keep Gemini on an OAuth-first policy.
fn google_openai_compat_from_env() -> Result<OpenAiCompatClient, ApiError> {
    let config = OpenAiCompatConfig::google();
    if openai_compat::has_custom_base_url(config) {
        return OpenAiCompatClient::from_env_optional_auth(config);
    }
    if crate::providers::google_auth::gemini_oauth_available() {
        return Ok(OpenAiCompatClient::google_oauth(config));
    }
    OpenAiCompatClient::from_env(config)
}

/// Build the OpenAI-compatible client for a user-defined provider. A provider
/// declaring `requires_auth: true` errors clearly when its key env is unset;
/// `false` lets a keyless self-host build with an empty key.
fn build_custom_client(
    custom: &providers::ResolvedCustomProvider,
) -> Result<OpenAiCompatClient, ApiError> {
    if custom.requires_auth {
        OpenAiCompatClient::from_env(custom.config)
    } else {
        OpenAiCompatClient::from_env_optional_auth(custom.config)
    }
}

/// The saved ChatGPT login as a request would carry it — refreshed first when
/// expired — for a caller outside the provider router that speaks to the
/// same backend (model discovery's `GET /backend-api/models`). `None` when
/// there is no ChatGPT login at all.
#[must_use]
pub fn resolve_openai_oauth_fresh() -> Option<OpenAiOAuthTokens> {
    load_fresh_openai_oauth()
}

/// [`resolve_openai_oauth_fresh`] for a caller that has to say why there is
/// no usable login: [`CredentialMiss::Absent`] when no ChatGPT login exists,
/// [`CredentialMiss::Unusable`] when one exists and could not be read, or
/// expired and would not refresh. The request path keeps its stale token (a
/// clear 401 beats a quiet api-key downgrade); a model list has nothing to
/// gain from a request it knows will be refused.
pub fn resolve_openai_oauth_explained() -> Result<OpenAiOAuthTokens, CredentialMiss> {
    match load_openai_login()? {
        OpenAiLogin::Fresh(tokens) => Ok(tokens),
        OpenAiLogin::Stale(_, why) => Err(CredentialMiss::Unusable(why)),
    }
}

/// Whether a ChatGPT login is kept where this process reads one — the codex
/// home the resolution table names, else zo's own store — whether or not it
/// can be used right now. A store that cannot be read is there all the same.
/// One file read, no refresh.
#[must_use]
pub fn openai_login_configured() -> bool {
    crate::oauth_store::load_openai_oauth_with_source().map_or(true, |found| found.is_some())
}

/// The ChatGPT login as found: usable now, or expired and not refreshable —
/// with its tokens, which the request path still sends, and why.
enum OpenAiLogin {
    Fresh(OpenAiOAuthTokens),
    Stale(OpenAiOAuthTokens, String),
}

/// Load the saved ChatGPT OAuth tokens, refreshing first when expired. Returns
/// `None` when no ChatGPT login exists so the caller falls back to the api-key
/// path. A refresh failure yields the existing (expired) tokens so the call can
/// surface a clear 401 rather than silently downgrading to the api-key path.
fn load_fresh_openai_oauth() -> Option<OpenAiOAuthTokens> {
    match load_openai_login().ok()? {
        OpenAiLogin::Fresh(tokens) | OpenAiLogin::Stale(tokens, _) => Some(tokens),
    }
}

fn load_openai_login() -> Result<OpenAiLogin, CredentialMiss> {
    let (tokens, source) = match crate::oauth_store::load_openai_oauth_with_source() {
        Ok(Some(found)) => found,
        Ok(None) => return Err(CredentialMiss::Absent),
        Err(error) => {
            return Err(CredentialMiss::Unusable(format!(
                "the ChatGPT login could not be read ({error})"
            )));
        }
    };
    if !openai_oauth_expired(&tokens) {
        return Ok(OpenAiLogin::Fresh(tokens));
    }
    let Some(refresh_token) = tokens.refresh_token.clone() else {
        return Ok(OpenAiLogin::Stale(
            tokens,
            format!(
                "the ChatGPT login expired and holds no refresh token — {}",
                openai_reconnect_hint(source)
            ),
        ));
    };
    // ChatGPT rotates refresh tokens exactly as Anthropic does, so a branch the
    // endpoint has rejected stays rejected. Without this gate an expired ChatGPT
    // login cost a doomed token round-trip on every request build — silently,
    // since the failure arm here deliberately serves the stale token so the call
    // surfaces one clear 401 instead of a confusing api-key downgrade.
    if crate::providers::refresh_gate::refresh_blocked(&refresh_token).is_some() {
        return Ok(OpenAiLogin::Stale(
            tokens,
            format!(
                "the ChatGPT login expired and its refresh was refused — {}",
                openai_reconnect_hint(source)
            ),
        ));
    }
    match run_blocking(crate::providers::openai_oauth::refresh_openai_tokens(
        &refresh_token,
    )) {
        Ok(mut refreshed) => {
            crate::providers::refresh_gate::record_success(&refresh_token);
            if refreshed.account_id.is_none() {
                refreshed.account_id = tokens.account_id;
            }
            if let Err(error) = crate::oauth_store::save_openai_oauth(&refreshed) {
                // A refresh that cannot be persisted is the worst outcome of all:
                // the rotation already happened server-side, so the token still on
                // disk is now the dead branch and the next process refreshes with
                // it. Say so rather than discarding the error.
                eprintln!(
                    "\x1b[33mwarning: refreshed ChatGPT token could not be saved ({error}); \
                     run `zo login openai` if the next request fails.\x1b[0m"
                );
            }
            Ok(OpenAiLogin::Fresh(refreshed))
        }
        Err(error) => {
            if crate::providers::refresh_gate::record_failure(&refresh_token, &error) {
                eprintln!(
                    "\x1b[33mChatGPT login can no longer be refreshed ({error}).\n  \
                     {}\x1b[0m",
                    openai_reconnect_hint(source)
                );
            }
            Ok(OpenAiLogin::Stale(
                tokens,
                format!(
                    "the ChatGPT login expired and could not be refreshed — {}",
                    openai_reconnect_hint(source)
                ),
            ))
        }
    }
}

/// What to do about a ChatGPT login that will not refresh, which depends on
/// WHOSE login it is.
///
/// zo's own store is the one that goes stale unnoticed: the window keeps its
/// account refreshed, so a person whose window is signed in has a working
/// account a metre away — and until t-5777 a zo started outside a pane never
/// looked at it, said "expired", and sent them to log in again for the second
/// time this month.
fn openai_reconnect_hint(source: crate::oauth_store::OpenAiAuthSource) -> &'static str {
    match source {
        crate::oauth_store::OpenAiAuthSource::OwnLogin => {
            "ZeroCode 창에 로그인돼 있으면 그 계정을 따라갑니다 — 창 밖이면 `zo login openai` \
             (또는 /login openai)."
        }
        crate::oauth_store::OpenAiAuthSource::CodexHome(_) => {
            "Run `zo login openai` (or /login openai) to reconnect."
        }
    }
}

/// Whether the saved ChatGPT token is within 60s of expiry (matching the
/// Anthropic OAuth buffer).
fn openai_oauth_expired(tokens: &OpenAiOAuthTokens) -> bool {
    tokens.expires_at.is_some_and(|expires_at| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        expires_at <= now + 60
    })
}

#[cfg(test)]
mod tests {
    /// 죽은 로그인을 고치는 길은 그 로그인이 **누구 것이냐**에 달렸다: 창의
    /// 계정이면 다시 로그인이 답이지만, 제 저장소가 죽은 것이라면 한 걸음
    /// 옆의 창에 살아 있는 계정이 있다 — 그 사실을 말하지 않는 문구가 사람을
    /// 한 달에 두 번 로그인시켰다(t-5777).
    #[test]
    fn a_dead_own_login_is_told_the_window_would_be_followed() {
        let own = super::openai_reconnect_hint(crate::oauth_store::OpenAiAuthSource::OwnLogin);
        assert!(own.contains("ZeroCode 창"), "{own}");
        assert!(own.contains("zo login openai"), "{own}");
        let borrowed = super::openai_reconnect_hint(crate::oauth_store::OpenAiAuthSource::CodexHome(
            crate::managed_account::CodexHomeSource::IdeManaged,
        ));
        assert!(
            !borrowed.contains("ZeroCode 창"),
            "the window's own account is already the one that failed: {borrowed}"
        );
    }

    /// C1 (t-6248): a model list asks the ChatGPT login whether it can be
    /// used and hears why not. No login is `Absent`; a login that expired
    /// with nothing to refresh it is `Unusable` and names the way back — while
    /// the request path still gets the stale token, whose 401 is clearer than
    /// a quiet fall to an API key.
    #[test]
    fn a_chatgpt_login_that_cannot_refresh_is_unusable_and_none_is_absent() {
        let _lock = crate::test_env_lock();
        let isolation = crate::test_env::CredentialEnvIsolation::empty();
        let _codex_home = EnvVarGuard::set("CODEX_HOME", None);
        crate::managed_account::clear();
        assert_eq!(
            super::resolve_openai_oauth_explained().err(),
            Some(super::CredentialMiss::Absent)
        );
        crate::oauth_store::save_openai_oauth(&core_types::OpenAiOAuthTokens {
            access_token: "stale-openai-access".to_string(),
            refresh_token: None,
            expires_at: Some(1),
            account_id: Some("acct".to_string()),
            scopes: Vec::new(),
        })
        .unwrap();
        let Some(super::CredentialMiss::Unusable(why)) = super::resolve_openai_oauth_explained().err() else {
            panic!("an expired login is there, so it is not absent");
        };
        assert!(why.contains("expired") && why.contains("zo login openai"), "{why}");
        assert!(!why.contains("stale-openai-access"), "no token in the words: {why}");
        assert_eq!(
            super::resolve_openai_oauth_fresh().map(|tokens| tokens.access_token).as_deref(),
            Some("stale-openai-access"),
            "the request path keeps its stale token"
        );
        drop(isolation);
    }

    /// C5 (t-6248): the Grok CLI's own login is an xAI credential, read
    /// through the one xAI resolution the model list reads too — the OAuth
    /// route speaks as it, and so does the automatic one when no key is set.
    /// An expired session is refused with the way back in, never refreshed.
    #[test]
    fn a_grok_cli_login_speaks_for_xai_when_no_key_is_set() {
        let _lock = crate::test_env_lock();
        let isolation = crate::test_env::CredentialEnvIsolation::empty();
        let grok_home = tempfile::tempdir().expect("a Grok home");
        let login_file = grok_home.path().join("auth.json");
        std::fs::write(
            &login_file,
            r#"{"https://auth.x.ai::acct-1":{"key":"grok-cli-session-token","expires_at":"2099-01-01T00:00:00.000000Z"}}"#,
        )
        .expect("a Grok login");
        let _grok = EnvVarGuard::set("GROK_HOME", grok_home.path().to_str());
        let _external = EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
        let _key = EnvVarGuard::set("XAI_API_KEY", None);
        let _base = EnvVarGuard::set("XAI_BASE_URL", None);

        let oauth = super::ProviderClient::from_provider_kind_with_auth_route(
            ProviderKind::Xai,
            super::AuthRoute::OAuth,
        )
        .expect("the Grok login is xAI's OAuth route");
        assert!(
            matches!(&oauth, super::ProviderClient::Xai(client) if client.speaks_with_bearer("grok-cli-session-token")),
            "the OAuth route speaks as the Grok login"
        );

        let _gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        let auto = super::ProviderClient::from_provider_kind_with_auth_route(
            ProviderKind::Xai,
            super::AuthRoute::Auto,
        )
        .expect("no key: the automatic route speaks as the Grok login");
        assert!(
            matches!(&auto, super::ProviderClient::Xai(client) if client.speaks_with_bearer("grok-cli-session-token")),
            "the automatic route speaks as the Grok login"
        );

        std::fs::write(
            &login_file,
            r#"{"https://auth.x.ai::acct-1":{"key":"grok-cli-session-token","expires_at":"2020-01-01T00:00:00Z"}}"#,
        )
        .expect("an expired Grok login");
        let refused = super::ProviderClient::from_provider_kind_with_auth_route(
            ProviderKind::Xai,
            super::AuthRoute::OAuth,
        )
        .expect_err("an expired session is refused");
        assert!(refused.to_string().contains("grok login"), "{refused}");
        drop(isolation);
    }

    use crate::providers::{
        EXPERIMENTAL_PROVIDERS_ENV, NON_CLAUDE_ADAPTERS_ENV, ProviderKind, detect_provider_kind,
        non_claude_adapters_enabled, resolve_model_alias,
    };

    struct EnvVarGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let original = std::env::var_os(key);
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn resolves_existing_and_grok_aliases() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(EXPERIMENTAL_PROVIDERS_ENV, Some("1"));
        assert_eq!(resolve_model_alias("opus"), "claude-opus-5");
        assert_eq!(resolve_model_alias("grok"), "grok-3");
    }

    #[test]
    fn provider_detection_prefers_model_family() {
        let _lock = crate::test_env_lock();
        let _gate = EnvVarGuard::set(EXPERIMENTAL_PROVIDERS_ENV, Some("1"));
        assert_eq!(detect_provider_kind("grok-3"), ProviderKind::Xai);
        assert_eq!(
            detect_provider_kind("claude-sonnet-4-6"),
            ProviderKind::Anthropic
        );
    }

    #[test]
    fn anthropic_client_rebuild_is_needed_when_the_managed_credentials_file_changes() {
        let _lock = crate::test_env_lock();
        let _isolation = crate::test_env::CredentialEnvIsolation::empty();
        let managed = tempfile::tempdir().expect("managed Claude home");
        let credentials = managed.path().join(".credentials.json");
        std::fs::write(
            &credentials,
            r#"{"claudeAiOauth":{"accessToken":"first","scopes":["user:inference"]}}"#,
        )
        .expect("first managed credentials");
        let _claude_home = EnvVarGuard::set(
            "CLAUDE_CONFIG_DIR",
            Some(managed.path().to_str().expect("utf8 managed home")),
        );
        let _disable_keychain = EnvVarGuard::set("ZO_DISABLE_KEYCHAIN", Some("1"));
        crate::providers::anthropic::keychain::invalidate_claude_code_keychain_cache();
        crate::providers::anthropic::resolve_claude_auth_fresh_detailed()
            .expect("initial managed auth");
        let client = super::ProviderClient::Anthropic(
            crate::providers::anthropic::AnthropicClient::new("first"),
        );
        assert!(!client.oauth_rebuild_needed());

        std::fs::write(
            &credentials,
            r#"{"claudeAiOauth":{"accessToken":"second-token","scopes":["user:inference"]}}"#,
        )
        .expect("switched managed credentials");

        assert!(client.oauth_rebuild_needed());
    }

    #[test]
    fn non_claude_adapters_default_to_disabled() {
        let _lock = crate::test_env_lock();
        let _legacy_gate = EnvVarGuard::set(EXPERIMENTAL_PROVIDERS_ENV, None);
        let _adapter_gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);
        // Implicit activation now also keys off provider base URLs, so a clean
        // default requires every provider credential/endpoint env to be unset.
        let _guards: Vec<EnvVarGuard> = [
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "GOOGLE_API_KEY",
            "GOOGLE_BASE_URL",
            "GOOGLE_ACCESS_TOKEN",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "HOME",
            "XAI_API_KEY",
            "XAI_BASE_URL",
            "OLLAMA_API_KEY",
            "OLLAMA_BASE_URL",
        ]
        .into_iter()
        .map(|key| EnvVarGuard::set(key, None))
        .collect();
        assert!(!non_claude_adapters_enabled());
    }

    #[test]
    fn google_code_assist_oauth_takes_priority_over_custom_base_url() {
        let _lock = crate::test_env_lock();
        let temp_home = std::env::temp_dir().join(format!(
            "zo-google-oauth-priority-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&temp_home);
        let temp_home_str = temp_home.to_str().expect("utf-8 temp path").to_string();

        let _config_home = EnvVarGuard::set("ZO_CONFIG_HOME", Some(&temp_home_str));
        let _zo_home = EnvVarGuard::set("ZO_HOME", None);
        let _disable_external = EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
        let _base = EnvVarGuard::set("GOOGLE_BASE_URL", Some("http://localhost:9999/v1"));
        let _key = EnvVarGuard::set("GOOGLE_API_KEY", None);

        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_secs()
            + 3600;
        crate::oauth_store::save_google_code_assist_oauth(&core_types::OAuthTokenSet {
            access_token: "access-token".to_string(),
            refresh_token: None,
            expires_at: Some(expires_at),
            scopes: Vec::new(),
        })
        .expect("save google oauth token");

        let client =
            super::ProviderClient::from_model_with_anthropic_auth("gemini-3-flash-preview", None)
                .expect("google client");
        assert!(matches!(client, super::ProviderClient::GeminiCodeAssist(_)));

        let _ = std::fs::remove_dir_all(&temp_home);
    }

    fn provider_error(
        result: Result<super::ProviderClient, crate::ApiError>,
        message: &str,
    ) -> crate::ApiError {
        let Err(error) = result else {
            panic!("{message}");
        };
        error
    }

    #[test]
    fn auth_route_serde_round_trips() {
        for (route, encoded) in [
            (super::AuthRoute::Auto, "\"auto\""),
            (super::AuthRoute::OAuth, "\"oauth\""),
            (super::AuthRoute::ApiKey, "\"api-key\""),
        ] {
            assert_eq!(serde_json::to_string(&route).unwrap(), encoded);
            assert_eq!(serde_json::from_str::<super::AuthRoute>(encoded).unwrap(), route);
        }
    }

    #[test]
    fn forced_oauth_routes_do_not_fall_back_to_api_keys() {
        let _lock = crate::test_env_lock();
        let temp_home = std::env::temp_dir().join(format!(
            "zo-forced-oauth-route-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&temp_home);
        std::fs::create_dir_all(&temp_home).unwrap();
        let temp_home_str = temp_home.to_str().expect("utf-8 temp path").to_string();
        let _config_home = EnvVarGuard::set("ZO_CONFIG_HOME", Some(&temp_home_str));
        let _zo_home = EnvVarGuard::set("ZO_HOME", Some(&temp_home_str));
        let _home = EnvVarGuard::set("HOME", Some(&temp_home_str));
        // This exercises zo's own credential store, and a handed-off `CODEX_HOME`
        // account now outranks it — the IDE exports one into every pane it opens.
        let _codex_home = EnvVarGuard::set("CODEX_HOME", None);
        let _disable_external = EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
        let _google_key = EnvVarGuard::set("GOOGLE_API_KEY", Some("google-test-key"));
        let _openai_key = EnvVarGuard::set("OPENAI_API_KEY", Some("openai-test-key"));

        let google_error = provider_error(
            super::ProviderClient::from_provider_kind_with_auth_route(
                ProviderKind::Google,
                super::AuthRoute::OAuth,
            ),
            "Google OAuth must not fall back to the API key",
        );
        assert!(matches!(
            google_error,
            crate::ApiError::MissingAuthRouteCredentials {
                provider: "Google",
                route: "oauth"
            }
        ));
        let openai_error = provider_error(
            super::ProviderClient::from_provider_kind_with_auth_route(
                ProviderKind::OpenAi,
                super::AuthRoute::OAuth,
            ),
            "OpenAI OAuth must not fall back to the API key",
        );
        assert!(matches!(
            openai_error,
            crate::ApiError::MissingAuthRouteCredentials {
                provider: "OpenAI",
                route: "oauth"
            }
        ));
        let _ = std::fs::remove_dir_all(temp_home);
    }

    #[test]
    fn forced_routes_select_exact_credentials_without_fallback() {
        let _lock = crate::test_env_lock();
        let temp_home = std::env::temp_dir().join(format!(
            "zo-forced-exact-route-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&temp_home);
        std::fs::create_dir_all(&temp_home).unwrap();
        let temp_home_str = temp_home.to_str().expect("utf-8 temp path").to_string();
        let _config_home = EnvVarGuard::set("ZO_CONFIG_HOME", Some(&temp_home_str));
        let _zo_home = EnvVarGuard::set("ZO_HOME", Some(&temp_home_str));
        let _home = EnvVarGuard::set("HOME", Some(&temp_home_str));
        // This exercises zo's own credential store, and a handed-off `CODEX_HOME`
        // account now outranks it — the IDE exports one into every pane it opens.
        let _codex_home = EnvVarGuard::set("CODEX_HOME", None);
        let _disable_external = EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
        let _google_key = EnvVarGuard::set("GOOGLE_API_KEY", Some("google-test-key"));
        let _openai_key = EnvVarGuard::set("OPENAI_API_KEY", Some("openai-test-key"));
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        crate::oauth_store::save_google_code_assist_oauth(&core_types::OAuthTokenSet {
            access_token: "google-oauth-token".to_string(),
            refresh_token: None,
            expires_at: Some(expires_at),
            scopes: Vec::new(),
        })
        .unwrap();
        crate::oauth_store::save_openai_oauth(&core_types::OpenAiOAuthTokens {
            access_token: "openai-oauth-token".to_string(),
            refresh_token: None,
            expires_at: Some(expires_at),
            account_id: Some("acct".to_string()),
            scopes: Vec::new(),
        })
        .unwrap();

        assert!(matches!(
            super::ProviderClient::from_provider_kind_with_auth_route(
                ProviderKind::Google,
                super::AuthRoute::ApiKey,
            )
            .unwrap(),
            super::ProviderClient::Google(_)
        ));
        assert!(matches!(
            super::ProviderClient::from_provider_kind_with_auth_route(
                ProviderKind::OpenAi,
                super::AuthRoute::ApiKey,
            )
            .unwrap(),
            super::ProviderClient::OpenAi(_)
        ));
        assert!(matches!(
            super::ProviderClient::from_provider_kind_with_auth_route(
                ProviderKind::Google,
                super::AuthRoute::OAuth,
            )
            .unwrap(),
            super::ProviderClient::GeminiCodeAssist(_)
        ));
        assert!(matches!(
            super::ProviderClient::from_provider_kind_with_auth_route(
                ProviderKind::OpenAi,
                super::AuthRoute::OAuth,
            )
            .unwrap(),
            super::ProviderClient::ChatGpt(_)
        ));

        std::env::remove_var("GOOGLE_API_KEY");
        let key_error = provider_error(
            super::ProviderClient::from_provider_kind_with_auth_route(
                ProviderKind::Google,
                super::AuthRoute::ApiKey,
            ),
            "Google API key route must not use saved OAuth",
        );
        assert!(matches!(
            key_error,
            crate::ApiError::MissingAuthRouteCredentials {
                provider: "Google",
                route: "api-key"
            }
        ));
        let _ = std::fs::remove_dir_all(temp_home);
    }

    #[test]
    fn unsupported_explicit_auth_route_is_rejected() {
        let unsupported = provider_error(
            super::ProviderClient::from_provider_kind_with_auth_route(
                ProviderKind::Ollama,
                super::AuthRoute::OAuth,
            ),
            "Ollama OAuth is unsupported",
        );
        assert!(matches!(
            unsupported,
            crate::ApiError::UnsupportedAuthRoute { .. }
        ));
    }

    #[test]
    fn oauth_rebuild_needed_only_when_saved_oauth_is_near_expiry() {
        let _lock = crate::test_env_lock();
        let temp_home = std::env::temp_dir().join(format!(
            "zo-oauth-rebuild-needed-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&temp_home);
        std::fs::create_dir_all(&temp_home).expect("temp home");
        let temp_home_str = temp_home.to_str().expect("utf-8 temp path").to_string();

        let _config_home = EnvVarGuard::set("ZO_CONFIG_HOME", Some(&temp_home_str));
        let _zo_home = EnvVarGuard::set("ZO_HOME", None);
        // As above: the pane's handed-off account must not shadow zo's own store.
        let _codex_home = EnvVarGuard::set("CODEX_HOME", None);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_secs();

        crate::oauth_store::save_google_code_assist_oauth(&core_types::OAuthTokenSet {
            access_token: "fresh-google-token".to_string(),
            refresh_token: Some("google-refresh".to_string()),
            expires_at: Some(now + 3600),
            scopes: Vec::new(),
        })
        .expect("save fresh google oauth");
        let google = super::ProviderClient::GeminiCodeAssist(
            crate::providers::gemini_code_assist::GeminiCodeAssistClient::new("fresh-google-token"),
        );
        assert!(
            !google.oauth_rebuild_needed(),
            "fresh Code Assist OAuth must not rebuild before every turn"
        );

        crate::oauth_store::save_google_code_assist_oauth(&core_types::OAuthTokenSet {
            access_token: "expired-google-token".to_string(),
            refresh_token: Some("google-refresh".to_string()),
            expires_at: Some(now.saturating_sub(1)),
            scopes: Vec::new(),
        })
        .expect("save expired google oauth");
        assert!(google.oauth_rebuild_needed());

        crate::oauth_store::save_openai_oauth(&core_types::OpenAiOAuthTokens {
            access_token: "fresh-openai-token".to_string(),
            refresh_token: Some("openai-refresh".to_string()),
            expires_at: Some(now + 3600),
            account_id: Some("acct".to_string()),
            scopes: Vec::new(),
        })
        .expect("save fresh openai oauth");
        let chatgpt = super::ProviderClient::ChatGpt(
            crate::providers::chatgpt_backend::ChatGptBackendClient::new(
                "fresh-openai-token",
                Some("acct".to_string()),
            ),
        );
        assert!(
            !chatgpt.oauth_rebuild_needed(),
            "fresh ChatGPT OAuth must not rebuild before every turn"
        );

        crate::oauth_store::save_openai_oauth(&core_types::OpenAiOAuthTokens {
            access_token: "expired-openai-token".to_string(),
            refresh_token: Some("openai-refresh".to_string()),
            expires_at: Some(now.saturating_sub(1)),
            account_id: Some("acct".to_string()),
            scopes: Vec::new(),
        })
        .expect("save expired openai oauth");
        assert!(chatgpt.oauth_rebuild_needed());

        let _ = std::fs::remove_dir_all(temp_home);
    }

    #[test]
    fn openai_compat_from_env_tolerates_missing_key_for_custom_base_url() {
        use crate::providers::openai_compat::OpenAiCompatConfig;
        let _lock = crate::test_env_lock();
        let _key = EnvVarGuard::set("OPENAI_API_KEY", None);

        // Official cloud endpoint, no key → clean missing-credentials error.
        let no_base = EnvVarGuard::set("OPENAI_BASE_URL", None);
        assert!(super::openai_compat_from_env(OpenAiCompatConfig::openai()).is_err());
        drop(no_base);

        // Self-hosted endpoint (custom base URL), no key → constructs with
        // optional auth instead of erroring.
        let _base = EnvVarGuard::set("OPENAI_BASE_URL", Some("http://localhost:8080/v1"));
        assert!(super::openai_compat_from_env(OpenAiCompatConfig::openai()).is_ok());
    }

    fn custom_provider(json: &str) -> crate::providers::ResolvedCustomProvider {
        let parsed = crate::providers::parse_custom_providers(&format!("[{json}]"))
            .expect("valid custom provider json");
        let custom = parsed.into_iter().next().expect("one provider");
        crate::providers::ResolvedCustomProvider::from(custom)
    }

    #[test]
    fn custom_grok_model_uses_custom_provider_before_xai_gate() {
        let _lock = crate::test_env_lock();
        let _legacy_gate = EnvVarGuard::set(crate::providers::EXPERIMENTAL_PROVIDERS_ENV, None);
        let _adapter_gate = EnvVarGuard::set(NON_CLAUDE_ADAPTERS_ENV, None);
        let _xai_key = EnvVarGuard::set("XAI_API_KEY", None);
        let _xai_base = EnvVarGuard::set("XAI_BASE_URL", None);
        crate::providers::refresh_custom_providers_from_json(
            r#"[{"name":"xai-custom","base_url":"https://api.x.ai/v1","models":["grok-4.5"],"requires_auth":false}]"#,
        )
        .expect("refresh custom provider");

        let client = super::ProviderClient::from_model("grok-4.5")
            .expect("custom grok model should bypass built-in xAI adapter gate");
        assert!(matches!(client, super::ProviderClient::OpenAi(_)));

        crate::providers::refresh_custom_providers_from_json("[]")
            .expect("restore empty custom providers");
    }

    #[test]
    fn build_custom_client_allows_keyless_self_host() {
        let _lock = crate::test_env_lock();
        // requires_auth:false → builds even with no API key env set.
        let provider = custom_provider(
            r#"{"name":"Local","base_url":"http://localhost:11434/v1",
                "models":["llama-3.3"],"requires_auth":false}"#,
        );
        assert!(super::build_custom_client(&provider).is_ok());
    }

    #[test]
    fn build_custom_client_errors_when_required_key_missing() {
        let _lock = crate::test_env_lock();
        let _key = EnvVarGuard::set("CUSTOM_PROVIDER_KEY", None);
        // requires_auth:true (default) + unset key env → clean error, no panic.
        let provider = custom_provider(
            r#"{"name":"Cloudish","base_url":"https://api.cloudish.example/v1",
                "auth_env":"CUSTOM_PROVIDER_KEY","models":["cloud-large"]}"#,
        );
        let error = super::build_custom_client(&provider).expect_err("missing custom key");
        let rendered = error.to_string();
        assert!(
            rendered.contains("CUSTOM_PROVIDER_KEY"),
            "custom provider missing-credential error must name auth_env: {rendered}"
        );
        assert!(
            !rendered.contains("export  before"),
            "custom provider missing-credential error must not render a blank export hint: {rendered}"
        );
        assert!(
            !rendered.contains("zo login") && !rendered.contains("/login"),
            "custom adapter missing-credential error must not suggest OAuth login: {rendered}"
        );
    }
}
