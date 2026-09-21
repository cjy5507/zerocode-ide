//! Google OAuth + Code Assist backend.
//!
//! The "Sign in with Google" path does **not** use the public Generative
//! Language OpenAI-compatible API key surface. `/login google` runs the
//! Antigravity IDE's own Google sign-in flow using Antigravity's built-in
//! installed-app OAuth client, so it needs no per-user setup — the same way zo
//! mirrors the OpenAI Codex CLI sign-in. It stores the resulting refresh token
//! locally and calls the Cloud Code backend the Antigravity clients use
//! (`daily-cloudcode-pa.googleapis.com`; see `DEFAULT_CODE_ASSIST_ENDPOINT`).
//!
//! The built-in client id and secret are installed-app credentials, which
//! Google documents as non-confidential — they ship inside (and are extractable
//! from) the desktop client (OAuth for native apps, RFC 8252 §8.5), the same
//! posture the Gemini Code Assist CLI takes with its own. Override both with the
//! `ZO_GEMINI_CODE_ASSIST_OAUTH_CLIENT_ID`/`_SECRET` env vars to sign in with a
//! caller-owned client instead.
//!
//! Note: the Code Assist compatibility metadata used by this backend may be
//! subject to additional Google terms. Google ADC remains available through
//! `zo login google-adc` without these Code Assist OAuth credentials.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::managed_account::external_credentials_disabled;
use core_types::session::ToolCallPairing;
use core_types::{OAuthAuthorizationRequest, OAuthConfig, OAuthTokenSet, PkceCodePair};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    EffortLevel, ImageSource, InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent,
    MessageRequest, MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock,
    ReasoningRequest, StreamEvent, SystemBlock, ToolChoice, ToolDefinition, ToolLedgerView, Usage,
};
// Generic `data: {json}` SSE frame splitter (handles multi-chunk frames and the
// O(n) resume scan). Named for the Responses backend it was written for, but the
// framing is provider-agnostic, so the Gemini SSE stream reuses it rather than
// re-deriving the same boundary handling.
use super::chatgpt_backend::ResponsesSseParser;
use super::crosses_restart_commit_boundary;

/// Whether surfacing `event` commits the Gemini stream — a restart past this
/// point would duplicate non-replay-safe output, so restarting must be disarmed.
///
/// Extends the shared [`crosses_restart_commit_boundary`] with a Gemini-specific
/// rule: a `functionCall` is surfaced as ONE atomic `ContentBlockStart` carrying
/// the COMPLETE tool arguments — Gemini never streams an `InputJsonDelta` the way
/// the OpenAI/Anthropic backends do (which is what commits a tool call there). So
/// the shared boundary alone never commits a Gemini tool call, and a retryable
/// fault after it would `restart()` the whole request and re-emit the call,
/// duplicating a side-effecting tool execution. A surfaced tool-call start is
/// therefore itself a commit point (true even for a no-arg call: the seeded args
/// are already complete).
fn gemini_stream_commit_boundary(event: &StreamEvent) -> bool {
    crosses_restart_commit_boundary(event)
        || matches!(
            event,
            StreamEvent::ContentBlockStart(start)
                if matches!(start.content_block, OutputContentBlock::ToolUse { .. })
        )
}

/// Antigravity IDE's built-in installed-app OAuth client, so `/login google`
/// runs Antigravity's own Google sign-in with no per-user setup. Installed-app
/// credentials are not confidential: Google documents the client secret as
/// embedded in (and extractable from) the desktop client (OAuth for native
/// apps, RFC 8252 §8.5) — the same posture the Gemini Code Assist CLI takes with
/// its own public client, and the same way zo ships the OpenAI Codex client.
/// Override both via the `ZO_GEMINI_CODE_ASSIST_OAUTH_CLIENT_ID`/`_SECRET` env
/// vars for a caller-owned client.
const ANTIGRAVITY_OAUTH_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const ANTIGRAVITY_OAUTH_CLIENT_SECRET: &str = "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";

/// Env var overriding `ANTIGRAVITY_OAUTH_CLIENT_ID` with a caller-owned
/// Google installed-app OAuth client ID.
pub const GEMINI_CODE_ASSIST_OAUTH_CLIENT_ID_ENV: &str =
    "ZO_GEMINI_CODE_ASSIST_OAUTH_CLIENT_ID";
/// Env var overriding the matching client secret.
pub const GEMINI_CODE_ASSIST_OAUTH_CLIENT_SECRET_ENV: &str =
    "ZO_GEMINI_CODE_ASSIST_OAUTH_CLIENT_SECRET";
const GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Fixed loopback callback port the Antigravity OAuth client is registered for.
/// Unlike Gemini CLI's ephemeral port, Antigravity's redirect URI is a fixed
/// `http://localhost:51121/oauth-callback`, so the local listener must bind it.
pub const ANTIGRAVITY_CALLBACK_PORT: u16 = 51121;

/// Antigravity IDE client version zo presents in the `User-Agent`. The
/// backend enforces a *minimum* client version and rejects anything older with
/// "This version of Antigravity is no longer supported" — that floor rises over
/// time, so this is pinned to a recent release (above the floor) and overridable
/// via `ZO_ANTIGRAVITY_VERSION` so a future bump needs no rebuild.
// The floor, not the truth: the registry gates its model list on this
// version (2.3.1 was served 25 models with 3.7 as one `-tiered` id; 2.9.x
// was served 31, the 3.7 rungs and `gemini-3.8-flash` among them, 3.8-high
// the default). 2.9.1 is the version the Antigravity CLI carries in its
// own binary (2026-09-03), so a floor of 2.9.1 is what agy itself
// presents; an installed IDE newer than this raises it, an older one does
// not lower it.
const ANTIGRAVITY_VERSION: &str = "2.9.1";
const ANTIGRAVITY_VERSION_ENV: &str = "ZO_ANTIGRAVITY_VERSION";
const ANTIGRAVITY_API_CLIENT: &str = "google-cloud-sdk vscode_cloudshelleditor/0.1";

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const USERINFO_EMAIL_SCOPE: &str = "https://www.googleapis.com/auth/userinfo.email";
const USERINFO_PROFILE_SCOPE: &str = "https://www.googleapis.com/auth/userinfo.profile";
const CCLOG_SCOPE: &str = "https://www.googleapis.com/auth/cclog";
const EXPERIMENTS_SCOPE: &str = "https://www.googleapis.com/auth/experimentsandconfigs";
const GEMINI_CODE_ASSIST_SCOPES: &[&str] = &[
    CLOUD_PLATFORM_SCOPE,
    USERINFO_EMAIL_SCOPE,
    USERINFO_PROFILE_SCOPE,
    CCLOG_SCOPE,
    EXPERIMENTS_SCOPE,
];

const CODE_ASSIST_ENDPOINT_ENV: &str = "CODE_ASSIST_ENDPOINT";
const CODE_ASSIST_API_VERSION_ENV: &str = "CODE_ASSIST_API_VERSION";
// The backend the Antigravity CLI and IDE themselves call. Measured
// 2026-09-03 with one consumer (Google AI Pro) login: on the prod host
// (`cloudcode-pa.googleapis.com`) `generateContent` answered 429
// RESOURCE_EXHAUSTED for every model — Claude and GPT rows with untouched
// quota included — while the same token, project and body on this host
// answered 200. The Antigravity CLI's own log names this host for
// `loadCodeAssist`, `fetchAvailableModels` and `streamGenerateContent`; the
// prod host is not where a consumer login's quota lives. Any other endpoint
// (prod, autopush) stays reachable through `CODE_ASSIST_ENDPOINT`.
const DEFAULT_CODE_ASSIST_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
const DEFAULT_CODE_ASSIST_API_VERSION: &str = "v1internal";
const TOKEN_EXPIRY_SKEW_SECS: u64 = 60;
const LOGIN_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(200);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(2);
const DEFAULT_MAX_RETRIES: u32 = 2;
/// Auto-refresh at the turn boundary must be quick: it runs before the
/// streaming loop's spinner (see `turn_controller::pump_draw_until`), so a long
/// stall reads as a TUI freeze. Tighter than the interactive-login timeout.
const REFRESH_TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
/// After a failed/timed-out auto-refresh, skip retrying for this long so a
/// single hung refresh can't re-stall every turn with a doomed retry.
const REFRESH_FAILURE_COOLDOWN_SECS: u64 = 60;
const LOGIN_SETUP_TOTAL_TIMEOUT: Duration = Duration::from_secs(45);
const SETUP_POLL_INTERVAL: Duration = Duration::from_secs(5);
const SETUP_MAX_POLLS: usize = 12;
/// Total budget for a single non-streaming `generateContent` call. The shared
/// HTTP client carries no blanket timeout (so true SSE streams can run long), so
/// a wedged blocking request would otherwise hang the turn forever. Generous
/// enough for a full high-effort completion, short enough to fail rather than
/// freeze.
const GENERATE_CONTENT_TIMEOUT: Duration = Duration::from_secs(300);

/// This process's Code Assist project, resolved via `loadCodeAssist` /
/// `onboardUser` or remembered from the credentials store, and the key it was
/// resolved for. Stable for that key, so re-resolving it per turn — or per
/// PROCESS — only adds latency; the store makes the second one go away too.
/// It answers only the key it holds: a client of another Google account (a
/// settings-pane sign-in rebuilds the pane's client with the new bearer) asks
/// afresh instead of being handed the account it left's project. `None` in
/// the project means "resolved: no project" (free tier); the outer `None`
/// means "not yet asked this process".
static RESOLVED_PROJECT: Mutex<Option<(ProjectKey, ResolvedProject)>> = Mutex::new(None);

/// What a Code Assist project resolution is the answer for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectKey {
    /// Whose project: the account the asking client's bearer belongs to.
    account: ProjectAccount,
    /// The project the configuration names (`GOOGLE_CLOUD_PROJECT` /
    /// `GOOGLE_CLOUD_PROJECT_ID`). It is configuration, so it wins over
    /// anything remembered: it is what `loadCodeAssist` is asked about, and a
    /// resolution made under another configuration is not its answer.
    configured: Option<String>,
}

/// The Google account a client's bearer belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProjectAccount {
    /// The saved Google grant the bearer was issued under, as
    /// [`crate::oauth_store::grant_fingerprint`] of its refresh token — Google
    /// refresh tokens do not rotate, so this names the login across
    /// processes, and the project is remembered under it.
    Grant(String),
    /// A bearer with no grant behind it (fingerprinted the same way): its
    /// project is remembered for this process only.
    Bearer(String),
}

impl ProjectAccount {
    fn of_bearer(access_token: &str) -> Self {
        Self::Bearer(crate::oauth_store::grant_fingerprint(access_token))
    }

    fn of_login(tokens: &OAuthTokenSet) -> Self {
        tokens.refresh_token.as_deref().map_or_else(
            || Self::of_bearer(&tokens.access_token),
            |refresh| Self::Grant(crate::oauth_store::grant_fingerprint(refresh)),
        )
    }

    /// The grant the store files this account's project under, if any.
    fn grant(&self) -> Option<&str> {
        match self {
            Self::Grant(grant) => Some(grant),
            Self::Bearer(_) => None,
        }
    }
}

fn lock_resolved_project() -> std::sync::MutexGuard<'static, Option<(ProjectKey, ResolvedProject)>> {
    RESOLVED_PROJECT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// This process's project for `key`, if it holds one for exactly that key.
fn cached_project(key: &ProjectKey) -> Option<ResolvedProject> {
    lock_resolved_project()
        .as_ref()
        .filter(|(cached, _)| cached == key)
        .map(|(_, resolved)| resolved.clone())
}

fn cache_project(key: &ProjectKey, resolved: &ResolvedProject) {
    *lock_resolved_project() = Some((key.clone(), resolved.clone()));
}

/// Where this process's project came from — a remembered one may be stale
/// (the backend can revoke or move it), so its first rejection is retried
/// once from a fresh resolution; a freshly resolved one is the truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectOrigin {
    Remembered,
    Resolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedProject {
    project: Option<String>,
    origin: ProjectOrigin,
}

/// The project an earlier process resolved for `key`; `None` when none is
/// remembered for exactly that key (or the account has no grant to file it under).
fn remembered_project(key: &ProjectKey) -> Option<ResolvedProject> {
    let grant = key.account.grant()?;
    match crate::oauth_store::load_google_code_assist_project(grant, key.configured.as_deref())
        .ok()?
    {
        crate::oauth_store::RememberedProject::Remembered(project) => Some(ResolvedProject {
            project,
            origin: ProjectOrigin::Remembered,
        }),
        crate::oauth_store::RememberedProject::NotRemembered => None,
    }
}

/// Whether the backend refused THIS request because of the project it named —
/// a permission or not-found answer, or a 400 that says so — as opposed to a
/// model, quota, or transport problem. Only such a refusal is worth a fresh
/// `loadCodeAssist` on a remembered project.
fn is_project_rejection(error: &ApiError) -> bool {
    match error {
        ApiError::Api { status, body, .. } => {
            status.as_u16() == 403
                || status.as_u16() == 404
                || (status.as_u16() == 400 && body.to_ascii_lowercase().contains("project"))
        }
        _ => false,
    }
}

/// Unix-seconds of the last failed auto-refresh (`0` = none). Drives the
/// post-failure refresh backoff in `load_fresh_oauth`.
static LAST_REFRESH_FAILURE_UNIX: AtomicU64 = AtomicU64::new(0);

/// Monotonic scope for zo-generated Gemini tool ids. Gemini Code Assist often
/// omits `functionCall.id`; a content-index-only fallback collides across
/// successive tool-loop streams (`gemini_tool_call_2` every time), so every
/// decoded response/stream gets its own process-local scope.
static GEMINI_TOOL_CALL_FALLBACK_SCOPE: AtomicU64 = AtomicU64::new(1);

fn next_gemini_tool_call_fallback_scope() -> u64 {
    GEMINI_TOOL_CALL_FALLBACK_SCOPE.fetch_add(1, Ordering::Relaxed)
}

fn gemini_tool_call_id(call: &Value, scope: u64, index: u32) -> String {
    call.get("id")
        .and_then(Value::as_str)
        .map_or_else(|| format!("gemini_tool_call_{scope}_{index}"), str::to_string)
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

impl GoogleTokenResponse {
    fn into_token_set(self, fallback_refresh: Option<String>) -> OAuthTokenSet {
        OAuthTokenSet {
            access_token: self.access_token,
            refresh_token: self.refresh_token.or(fallback_refresh),
            expires_at: self
                .expires_in
                .map(|seconds| now_unix().saturating_add(seconds)),
            scopes: self.scope.map_or_else(
                || {
                    GEMINI_CODE_ASSIST_SCOPES
                        .iter()
                        .map(|scope| (*scope).to_string())
                        .collect()
                },
                |scope| scope.split_whitespace().map(str::to_string).collect(),
            ),
        }
    }
}

/// Trim a credential env value, dropping it when unset or blank.
fn non_blank_credential(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Resolve the OAuth client `(id, optional secret)` as an atomic pair.
///
/// No id override → Antigravity's built-in installed-app client (id + secret),
/// so `/login google` runs Antigravity's sign-in with zero setup. A caller-owned
/// id override is paired only with its own secret override (or none, for a
/// public PKCE client) — never with the built-in secret, so Google never
/// receives a mismatched client. Pure over its inputs for testability.
fn resolve_client(
    id_override: Option<String>,
    secret_override: Option<String>,
) -> (String, Option<String>) {
    match non_blank_credential(id_override) {
        Some(client_id) => (client_id, non_blank_credential(secret_override)),
        None => (
            ANTIGRAVITY_OAUTH_CLIENT_ID.to_string(),
            Some(ANTIGRAVITY_OAUTH_CLIENT_SECRET.to_string()),
        ),
    }
}

/// The OAuth client sent on sign-in and token refresh: the env override pair,
/// else the built-in Antigravity client. See [`resolve_client`].
fn oauth_client() -> (String, Option<String>) {
    resolve_client(
        std::env::var(GEMINI_CODE_ASSIST_OAUTH_CLIENT_ID_ENV).ok(),
        std::env::var(GEMINI_CODE_ASSIST_OAUTH_CLIENT_SECRET_ENV).ok(),
    )
}

fn oauth_config_for_client(client_id: String) -> OAuthConfig {
    OAuthConfig {
        client_id,
        authorize_url: GOOGLE_AUTHORIZE_URL.to_string(),
        token_url: GOOGLE_TOKEN_URL.to_string(),
        callback_port: Some(ANTIGRAVITY_CALLBACK_PORT),
        manual_redirect_url: None,
        scopes: GEMINI_CODE_ASSIST_SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .collect(),
        client_secret: None,
    }
}

/// Resolve Gemini Code Assist OAuth configuration: the env-override client id,
/// else the built-in Antigravity client. The client secret is not needed to
/// build the authorize URL (PKCE covers the redirect); it is sent only at the
/// token endpoint by [`exchange_code`]/[`refresh_tokens`].
pub fn oauth_config() -> Result<OAuthConfig, ApiError> {
    let (client_id, _) = oauth_client();
    Ok(oauth_config_for_client(client_id))
}

/// Build the loopback redirect URI used by the Code Assist login flow.
#[must_use]
pub fn redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}/oauth-callback")
}

/// Build the browser authorization URL for Gemini Code Assist OAuth.
#[must_use]
pub fn authorize_url(
    config: &OAuthConfig,
    redirect_uri: &str,
    state: impl Into<String>,
    pkce: &PkceCodePair,
) -> String {
    OAuthAuthorizationRequest::from_config(config, redirect_uri, state, pkce)
        .with_extra_param("access_type", "offline")
        // Keep re-login reliable: Google may omit refresh_token on repeat
        // consent-less auth responses, while Zo needs a durable token.
        .with_extra_param("prompt", "consent")
        .build_url()
}

/// Exchange a Google authorization code for a Code Assist OAuth token set.
pub async fn exchange_code(
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenSet, ApiError> {
    let (client_id, client_secret) = oauth_client();
    let mut params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("client_id", client_id),
        ("code_verifier", code_verifier.to_string()),
    ];
    // Installed-app clients send their secret at the token endpoint (PKCE also
    // covers the redirect); a caller-owned public PKCE client omits it.
    if let Some(client_secret) = client_secret {
        params.push(("client_secret", client_secret));
    }
    let fallback_refresh = crate::oauth_store::load_google_code_assist_oauth()
        .ok()
        .flatten()
        .and_then(|tokens| tokens.refresh_token);
    let tokens = post_token_form(&params, LOGIN_HTTP_TIMEOUT)
        .await?
        .into_token_set(fallback_refresh);
    if tokens.refresh_token.is_none() {
        return Err(ApiError::Auth(
            "Google OAuth response carried no refresh_token; retry `/login google` so Zo can request offline access/consent".into(),
        ));
    }
    Ok(tokens)
}

/// Refresh a saved Gemini Code Assist OAuth token.
pub async fn refresh_tokens(refresh_token: &str) -> Result<OAuthTokenSet, ApiError> {
    let (client_id, client_secret) = oauth_client();
    let mut params = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
        ("client_id", client_id),
        ("scope", GEMINI_CODE_ASSIST_SCOPES.join(" ")),
    ];
    // Installed-app client refreshes with its secret; a caller-owned public
    // PKCE client omits it.
    if let Some(client_secret) = client_secret {
        params.push(("client_secret", client_secret));
    }
    Ok(post_token_form(&params, REFRESH_TOKEN_TIMEOUT)
        .await?
        .into_token_set(Some(refresh_token.to_string())))
}

async fn post_token_form(
    params: &[(&str, String)],
    timeout: Duration,
) -> Result<GoogleTokenResponse, ApiError> {
    let response = super::shared_http_client()
        .post(GOOGLE_TOKEN_URL)
        .timeout(timeout)
        .header("content-type", "application/x-www-form-urlencoded")
        .form(params)
        .send()
        .await
        .map_err(ApiError::from)?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::Api {
            status,
            error_type: None,
            message: google_error_message(&body),
            body,
            retryable: false,
            retry_after: None,
        });
    }
    response
        .json::<GoogleTokenResponse>()
        .await
        .map_err(ApiError::from)
}

/// Whether a saved Gemini Code Assist OAuth token exists.
#[must_use]
pub fn oauth_present() -> bool {
    !external_credentials_disabled()
        && crate::oauth_store::load_google_code_assist_oauth()
            .ok()
            .flatten()
            .is_some()
}

/// Whether a token is expired or close enough to expiry that refresh is safer.
#[must_use]
pub fn token_expired(tokens: &OAuthTokenSet) -> bool {
    tokens
        .expires_at
        .is_some_and(|expires_at| now_unix().saturating_add(TOKEN_EXPIRY_SKEW_SECS) >= expires_at)
}

/// Load saved Gemini Code Assist OAuth tokens, refreshing first when possible.
#[must_use]
pub fn load_fresh_oauth() -> Option<OAuthTokenSet> {
    if external_credentials_disabled() {
        return None;
    }
    let tokens = crate::oauth_store::load_google_code_assist_oauth()
        .ok()
        .flatten()?;
    if !token_expired(&tokens) {
        return Some(tokens);
    }
    let Some(refresh_token) = tokens.refresh_token.clone() else {
        return Some(tokens);
    };
    // Backoff: if a recent auto-refresh failed or timed out, don't retry yet —
    // serve the (expired) token so one hung refresh doesn't re-stall every turn
    // with a doomed retry. A genuine expiry still surfaces downstream as a 401.
    // The shared gate additionally retires a branch the endpoint *rejected*: a
    // revoked Google grant never recovers, so retrying it once a minute forever
    // is pure latency on the credential path.
    if refresh_in_backoff() || crate::providers::refresh_gate::refresh_blocked(&refresh_token).is_some()
    {
        return Some(tokens);
    }
    match crate::sync_bridge::run_blocking(refresh_tokens(&refresh_token)) {
        Ok(mut refreshed) => {
            clear_refresh_failure();
            crate::providers::refresh_gate::record_success(&refresh_token);
            if refreshed.refresh_token.is_none() {
                refreshed.refresh_token = tokens.refresh_token;
            }
            if let Err(error) = crate::oauth_store::save_google_code_assist_oauth(&refreshed) {
                eprintln!(
                    "\x1b[33mwarning: refreshed Gemini token could not be saved ({error}); \
                     run `zo login google` if the next request fails.\x1b[0m"
                );
            }
            Some(refreshed)
        }
        Err(error) => {
            mark_refresh_failure();
            if crate::providers::refresh_gate::record_failure(&refresh_token, &error) {
                eprintln!(
                    "\x1b[33mGemini login can no longer be refreshed ({error}).\n  \
                     Run `zo login google` (or /login google) to reconnect.\x1b[0m"
                );
            }
            Some(tokens)
        }
    }
}

/// `true` while inside the post-failure refresh backoff window.
fn refresh_in_backoff() -> bool {
    let last = LAST_REFRESH_FAILURE_UNIX.load(Ordering::Relaxed);
    last != 0 && now_unix().saturating_sub(last) < REFRESH_FAILURE_COOLDOWN_SECS
}

fn mark_refresh_failure() {
    LAST_REFRESH_FAILURE_UNIX.store(now_unix(), Ordering::Relaxed);
}

fn clear_refresh_failure() {
    LAST_REFRESH_FAILURE_UNIX.store(0, Ordering::Relaxed);
}

#[derive(Debug, Clone)]
pub struct GeminiCodeAssistClient {
    http: reqwest::Client,
    access_token: String,
    /// Whose Code Assist project this client asks for — fixed with its bearer.
    account: ProjectAccount,
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl GeminiCodeAssistClient {
    /// A client for a bare bearer. Its project is remembered for this process
    /// only; a client built from a saved login ([`Self::from_oauth`]) also
    /// finds the one an earlier process resolved.
    #[must_use]
    pub fn new(access_token: impl Into<String>) -> Self {
        let access_token = access_token.into();
        let account = ProjectAccount::of_bearer(&access_token);
        Self::for_account(access_token, account)
    }

    /// A client for the Google login `tokens` came from: its bearer, and the
    /// grant its Code Assist project is remembered under across processes.
    #[must_use]
    pub fn from_oauth(tokens: &OAuthTokenSet) -> Self {
        Self::for_account(tokens.access_token.clone(), ProjectAccount::of_login(tokens))
    }

    fn for_account(access_token: String, account: ProjectAccount) -> Self {
        Self {
            http: super::shared_http_client(),
            access_token,
            account,
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
        }
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
        let response = self
            .with_project(|project_id| async move {
                let payload = build_generate_content_request(request, project_id.as_deref());
                // generateContent is non-streaming here, so a stalled connection
                // would hang the turn forever (the shared client has no blanket
                // timeout — that is reserved for true SSE streams). Bound it with
                // a generous total timeout instead, so a wedged request fails
                // cleanly rather than freezing the TUI.
                self.post_json_with_timeout::<Value>(
                    "generateContent",
                    &payload,
                    Some(GENERATE_CONTENT_TIMEOUT),
                )
                .await
                .map_err(|error| request_not_found_context(error, &request.model))
            })
            .await?;
        normalize_generate_content_response(request, &response)
    }

    /// Run `send` with this client's Code Assist project. A remembered project
    /// the backend now refuses (revoked, moved) is forgotten, re-resolved once,
    /// and `send` retried; a fresh resolution's refusal is the answer. Every
    /// request that names a project goes through here, streaming or not — a
    /// process whose Gemini traffic is non-streaming only would otherwise fail
    /// every call and never heal the record for the next process.
    async fn with_project<T, F, Fut>(&self, mut send: F) -> Result<T, ApiError>
    where
        F: FnMut(Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<T, ApiError>>,
    {
        let key = self.project_key()?;
        let mut resolved = self.resolved_project(&key).await?;
        let mut retried = false;
        loop {
            match send(resolved.project.clone()).await {
                Ok(value) => return Ok(value),
                Err(error)
                    if !retried
                        && resolved.origin == ProjectOrigin::Remembered
                        && is_project_rejection(&error) =>
                {
                    retried = true;
                    Self::forget_project(&key);
                    resolved = self.resolve_project_afresh(&key).await?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// The Antigravity backend's serving registry for this account: every
    /// model id it accepts, with display names, token limits, and quota state.
    /// Catalog discovery reads it so a Gemini release the shipped catalog never
    /// heard of (`gemini-3.7-flash-tiered`) reaches the model list without a
    /// new binary — the registry, not a snapshot of it, is the serving list.
    pub async fn fetch_available_models(&self) -> Result<Value, ApiError> {
        self.post_json_with_timeout::<Value>(
            "fetchAvailableModels",
            &json!({}),
            Some(LOGIN_HTTP_TIMEOUT),
        )
        .await
    }

    /// What this client's project resolution is the answer for, read from the
    /// configuration now — an invalid configured project is refused before
    /// anything remembered could stand in for it.
    fn project_key(&self) -> Result<ProjectKey, ApiError> {
        Ok(ProjectKey {
            account: self.account.clone(),
            configured: google_cloud_project()?,
        })
    }

    /// The Code Assist project for `key`, resolved once per process and
    /// account. `loadCodeAssist` / `onboardUser` is stable for the key, so
    /// caching it avoids re-running that round-trip on every turn (a per-turn
    /// latency that read as a stall). Only a successful resolution is cached;
    /// a failure propagates and is retried.
    async fn resolved_project(&self, key: &ProjectKey) -> Result<ResolvedProject, ApiError> {
        if let Some(cached) = cached_project(key) {
            return Ok(cached);
        }
        if let Some(resolved) = remembered_project(key) {
            cache_project(key, &resolved);
            return Ok(resolved);
        }
        self.resolve_project_afresh(key).await
    }

    /// `loadCodeAssist` / `onboardUser` now, then remember the answer for this
    /// process and — under the account's grant — for the next one.
    async fn resolve_project_afresh(&self, key: &ProjectKey) -> Result<ResolvedProject, ApiError> {
        let project = self.setup_user_for(key.configured.clone()).await?;
        let resolved = ResolvedProject {
            project: project.clone(),
            origin: ProjectOrigin::Resolved,
        };
        cache_project(key, &resolved);
        if let Some(grant) = key.account.grant() {
            // Best effort: a store that cannot be written costs the next
            // process one round-trip, not this request.
            let _ = crate::oauth_store::save_google_code_assist_project(
                grant,
                key.configured.as_deref(),
                project.as_deref(),
            );
        }
        Ok(resolved)
    }

    /// Drop the remembered project for `key` (memory and store): the backend refused it.
    fn forget_project(key: &ProjectKey) {
        let mut slot = lock_resolved_project();
        if slot.as_ref().is_some_and(|(cached, _)| cached == key) {
            *slot = None;
        }
        drop(slot);
        let _ = crate::oauth_store::clear_google_code_assist_project();
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<GeminiCodeAssistStream, ApiError> {
        let model = request.model.as_str();
        self.with_project(|project_id| async move {
            self.open_stream_with_project(request, project_id.as_deref(), model)
                .await
        })
        .await
    }

    async fn open_stream_with_project(
        &self,
        request: &MessageRequest,
        project_id: Option<&str>,
        model: &str,
    ) -> Result<GeminiCodeAssistStream, ApiError> {
        let model = model.to_string();
        let project_id = project_id.map(str::to_string);
        // Build *and* serialize the request body off the async executor thread.
        // For a large post-fan-out context the `serde_json::Value` tree
        // construction (`build_generate_content_request`) plus JSON serialization
        // are hundreds of ms of pure CPU; run inline on the turn future they
        // starve `drive_turn`'s `select!` (frozen spinner/input). Cloning the
        // request is a cheap memcpy next to that build/serialize work, so the
        // async thread is freed and `render_tick` keeps painting.
        let request = request.clone();
        let body = tokio::task::spawn_blocking(move || {
            serde_json::to_vec(&build_generate_content_request(
                &request,
                project_id.as_deref(),
            ))
        })
        .await
        .map_err(|err| {
            ApiError::Io(std::io::Error::other(format!(
                "gemini payload build task panicked: {err}"
            )))
        })??;
        // True server-sent streaming: `streamGenerateContent?alt=sse` emits the
        // candidate incrementally (text token-by-token, functionCalls near the
        // end), so output surfaces as it is generated instead of arriving in one
        // burst once the whole reply is built. The connection stays open for the
        // turn, so it carries no blanket timeout — the per-chunk idle budget in
        // `next_event` bounds a wedged stream instead.
        let url = format!("{}?alt=sse", method_url("streamGenerateContent"));
        let response = self
            .open_stream_response(&url, body.clone(), &model)
            .await?;
        Ok(GeminiCodeAssistStream::new(
            response,
            self.clone(),
            url,
            body,
            model,
        ))
    }

    async fn open_stream_response(
        &self,
        url: &str,
        body: Vec<u8>,
        requested_model: &str,
    ) -> Result<reqwest::Response, ApiError> {
        super::debug_dump(
            GEMINI_DEBUG_ENV,
            "gemini",
            "request",
            &String::from_utf8_lossy(&body),
        );
        let builder = self
            .http
            .post(url)
            .bearer_auth(&self.access_token)
            .header("content-type", "application/json")
            // Negotiate SSE explicitly. Without `Accept: text/event-stream` the
            // Code Assist backend buffers the whole reply and flushes it as a
            // single frame (so it still arrives "all at once" despite `alt=sse`,
            // leaving the TUI frozen until the full turn lands); with it, the
            // candidate streams chunk by chunk and `next_event` surfaces tokens
            // as they are generated.
            .header("accept", "text/event-stream")
            .body(body);
        let response = antigravity_headers(builder)
            .send()
            .await
            .map_err(ApiError::from)?;
        let status = response.status();
        if !status.is_success() {
            let error = api_error_from_response(status, response).await;
            return Err(request_not_found_context(error, requested_model));
        }
        Ok(response)
    }

    fn backoff_for_attempt(&self, attempt: u32) -> Result<Duration, ApiError> {
        super::backoff_for_attempt(attempt, self.initial_backoff, self.max_backoff)
    }

    async fn setup_user(&self) -> Result<Option<String>, ApiError> {
        self.setup_user_for(google_cloud_project()?).await
    }

    /// `loadCodeAssist` (and `onboardUser` when the account needs it) asked
    /// about the configured project `project_id`.
    async fn setup_user_for(&self, project_id: Option<String>) -> Result<Option<String>, ApiError> {
        let metadata = client_metadata(project_id.as_deref());
        let request = json!({
            "cloudaicompanionProject": project_id,
            "metadata": metadata,
        });
        let load = self
            .post_json_with_timeout::<Value>("loadCodeAssist", &request, Some(LOGIN_HTTP_TIMEOUT))
            .await?;
        setup_user_from_load_response(&load, project_id, self).await
    }

    async fn post_json_with_timeout<T: DeserializeOwned>(
        &self,
        method: &str,
        payload: &Value,
        timeout: Option<Duration>,
    ) -> Result<T, ApiError> {
        let builder = self
            .http
            .post(method_url(method))
            .bearer_auth(&self.access_token)
            .header("content-type", "application/json");
        let mut request = antigravity_headers(builder).json(payload);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await.map_err(ApiError::from)?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_error_from_response(status, response).await);
        }
        response.json::<T>().await.map_err(ApiError::from)
    }

    async fn get_json_with_timeout<T: DeserializeOwned>(
        &self,
        url: String,
        timeout: Option<Duration>,
    ) -> Result<T, ApiError> {
        let builder = self
            .http
            .get(url)
            .bearer_auth(&self.access_token)
            .header("content-type", "application/json");
        let mut request = antigravity_headers(builder);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await.map_err(ApiError::from)?;
        let status = response.status();
        if !status.is_success() {
            return Err(api_error_from_response(status, response).await);
        }
        response.json::<T>().await.map_err(ApiError::from)
    }
}

/// A live Server-Sent-Events stream over `streamGenerateContent`. Each SSE frame
/// is a partial `GenerateContentResponse` (wrapped in `{"response": ...}` by the
/// Code Assist backend); [`Self::next_event`] decodes frames incrementally into
/// zo's Anthropic-shaped [`StreamEvent`]s as the bytes arrive, so a turn
/// surfaces output while the model is still generating.
#[derive(Debug)]
pub struct GeminiCodeAssistStream {
    response: reqwest::Response,
    parser: ResponsesSseParser,
    state: GeminiStreamState,
    pending: VecDeque<StreamEvent>,
    done: bool,
    client: GeminiCodeAssistClient,
    url: String,
    body: Vec<u8>,
    model: String,
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

impl GeminiCodeAssistStream {
    fn new(
        response: reqwest::Response,
        client: GeminiCodeAssistClient,
        url: String,
        body: Vec<u8>,
        model: String,
    ) -> Self {
        Self {
            response,
            parser: ResponsesSseParser::new(),
            state: GeminiStreamState::new(model.clone()),
            pending: VecDeque::new(),
            done: false,
            client,
            url,
            body,
            model,
            committed: false,
            restart_attempts: 0,
            restart_window_start: None,
            max_restart_wallclock: super::DEFAULT_MAX_RESTART_WALLCLOCK,
            retry_notice: super::StreamRetryNotifier::none(),
        }
    }

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
        self.state.request_id.as_deref()
    }

    /// Pull the next decoded event, reading more of the SSE body when the local
    /// queue drains. A per-chunk idle timeout (shared with the other streaming
    /// backends) turns a wedged connection into a retryable error rather than a
    /// hung turn; each received chunk resets the budget, so a long-but-active
    /// stream is never cut.
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        let idle_timeout = super::stream_idle_timeout();
        loop {
            if let Some(event) = self.pending.pop_front() {
                if gemini_stream_commit_boundary(&event) {
                    self.committed = true;
                }
                return Ok(Some(event));
            }
            if self.done {
                return Ok(None);
            }
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
                Err(error) => return Err(self.wrap_restart_exhaustion(error)),
            };
            if let Some(chunk) = chunk {
                for value in self.parser.push(&chunk)? {
                    self.pending.extend(self.state.ingest(&value));
                }
            } else {
                // End of the SSE body: flush any open text block, the closing
                // delta (stop reason + signature), and the stop event.
                self.pending.extend(self.state.finish());
                self.done = true;
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

    fn retries_exhausted(&self, error: ApiError) -> ApiError {
        ApiError::RetriesExhausted {
            attempts: self.restart_attempts.saturating_add(1),
            last_error: Box::new(error),
        }
    }

    fn wrap_restart_exhaustion(&self, error: ApiError) -> ApiError {
        let attempts_spent = self.restart_attempts >= self.client.max_retries;
        let wallclock_spent = self
            .restart_window_start
            .is_some_and(|start| start.elapsed() >= self.max_restart_wallclock);
        if !self.committed && error.is_retryable() && (attempts_spent || wallclock_spent) {
            self.retries_exhausted(error)
        } else {
            error
        }
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
        // Stamp a fresh `requestId` on the replayed payload: the id names one
        // request *attempt* for tracing, and re-sending the aborted attempt's
        // id would make the retry indistinguishable from the original upstream
        // (or collide, if Code Assist ever enforces uniqueness).
        self.response = match tokio::time::timeout(
            remaining,
            self.client.open_stream_response(
                &self.url,
                body_with_fresh_request_id(&self.body),
                &self.model,
            ),
        )
        .await
        {
            Ok(response) => response?,
            Err(_elapsed) => {
                return Err(
                    self.retries_exhausted(ApiError::stream_restart_timeout(remaining))
                );
            }
        };
        self.parser = ResponsesSseParser::new();
        self.state = GeminiStreamState::new(self.model.clone());
        self.pending.clear();
        self.done = false;
        Ok(())
    }
}

/// Per-stream decode state: turns the incremental Code Assist frames into the
/// `message_start` → block deltas → `message_delta` → `message_stop` shape the
/// runtime parser expects. Initial text uses index 0; if Gemini emits text after
/// a tool row, that later text uses a fresh append-only block index so visible
/// terminal rows stay stable instead of being rewritten. Each functionCall is a
/// self-contained block (start + args + stop) at the next dynamic index. The
/// end-of-turn `thoughtSignature`s ride on the closing `message_delta` (they are
/// only known once the late functionCall parts arrive).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GeminiMessageStartState {
    Pending,
    Started,
}

impl GeminiMessageStartState {
    const fn is_pending(self) -> bool {
        matches!(self, Self::Pending)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GeminiBlockState {
    Closed,
    Open,
}

impl GeminiBlockState {
    const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VisibleOutputState {
    None,
    Started,
}

impl VisibleOutputState {
    const fn has_started(self) -> bool {
        matches!(self, Self::Started)
    }
}

#[derive(Debug)]
struct GeminiStreamState {
    model: String,
    request_id: Option<String>,
    message_start: GeminiMessageStartState,
    text_block: GeminiBlockState,
    text_block_index: u32,
    /// Once answer prose or a tool row has been surfaced, any later Gemini
    /// thought part is stale for this stream. Keep this separate from
    /// `text_block`: a functionCall deliberately closes prose before the tool row,
    /// but that must not allow a late thought to surface reasoning afterward.
    visible_output: VisibleOutputState,
    /// Per-stream scope used only for zo-generated fallback tool ids. Keeps
    /// id-less Gemini functionCalls unique across separate tool-loop streams.
    tool_call_fallback_scope: u64,
    /// Next append-only content index for tool calls and any text that arrives
    /// after a tool row. This keeps terminal output stable: settled rows are
    /// never reused or rewritten; later visible content appends behind the row
    /// the user is already watching.
    next_content_index: u32,
    /// Whether a streamed `Thinking` block (Gemini thought summary) is currently
    /// open. Thought parts carry `"thought": true` and stream before the answer,
    /// so surfacing them as reasoning is what fills the otherwise-blank wait.
    thought_block: GeminiBlockState,
    fc_signatures: Vec<Option<String>>,
    stop_reason: Option<String>,
    usage: Usage,
}

/// Content-block index reserved for the single coalesced answer-text block.
const GEMINI_TEXT_BLOCK_INDEX: u32 = 0;
/// Content-block index reserved for the single coalesced thought-summary block.
/// Distinct from text so a reasoning delta never splices into the answer.
const GEMINI_THOUGHT_BLOCK_INDEX: u32 = 1;

impl GeminiStreamState {
    fn new(model: String) -> Self {
        Self {
            model,
            request_id: None,
            message_start: GeminiMessageStartState::Pending,
            text_block: GeminiBlockState::Closed,
            text_block_index: GEMINI_TEXT_BLOCK_INDEX,
            visible_output: VisibleOutputState::None,
            tool_call_fallback_scope: next_gemini_tool_call_fallback_scope(),
            thought_block: GeminiBlockState::Closed,
            // 0/1 are reserved for the first text/thought blocks; later content
            // appends after them so text/tool deltas never collide.
            next_content_index: 2,
            fc_signatures: Vec::new(),
            stop_reason: None,
            usage: Usage {
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens: 0,
                output_tokens_details: None,
            },
        }
    }

    /// Emit a streamed answer-text delta, opening a text block on first use.
    /// The initial prose still uses the reserved text index. If Gemini sends
    /// prose after a tool row, preserve it in a fresh append-only block instead
    /// of reusing index 0 or dropping content.
    fn push_text_delta(&mut self, events: &mut Vec<StreamEvent>, text: &str) {
        if !self.text_block.is_open() {
            let index = if self.visible_output.has_started() {
                let index = self.next_content_index;
                self.next_content_index += 1;
                index
            } else {
                GEMINI_TEXT_BLOCK_INDEX
            };
            self.text_block_index = index;
            self.visible_output = VisibleOutputState::Started;
            // The answer has begun — settle the thought block first (mirrors the
            // functionCall path). Other providers close their reasoning block
            // before prose, so it reads as `done`; leaving Gemini's open while
            // the answer streams keeps a `Reasoning { done: false }` directly
            // above live prose, which makes the transcript re-measure the whole
            // reasoning-and-answer region on *every* answer token
            // (`streaming_reasoning_hidden_by_tail_prose_idx`) — a continuous
            // re-layout the user sees as flicker.
            if self.thought_block.is_open() {
                events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                    index: GEMINI_THOUGHT_BLOCK_INDEX,
                }));
                self.thought_block = GeminiBlockState::Closed;
            }
            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index,
                content_block: OutputContentBlock::Text {
                    text: String::new(),
                },
            }));
            self.text_block = GeminiBlockState::Open;
        }
        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: self.text_block_index,
            delta: ContentBlockDelta::TextDelta {
                text: text.to_string(),
            },
        }));
    }

    /// Emit a streamed thought-summary delta as a `Thinking`/`Reasoning` block,
    /// opening the coalesced thought block on first use. This is what fills the
    /// wait with visible reasoning instead of a blank screen.
    fn push_thought_delta(&mut self, events: &mut Vec<StreamEvent>, text: &str) {
        // Once answer prose or a tool row has surfaced, any later thought is
        // stale for this stream. A late thought part must NOT surface reasoning
        // after visible output: it would create a live reasoning block in the
        // tail region, changing suppression/height calculations and forcing the
        // transcript to re-measure output the user is already watching. Gemini's
        // contract is thoughts-stream-before-answer/tool, so thoughts arriving
        // after visible output started are dropped.
        if self.visible_output.has_started() {
            return;
        }
        if !self.thought_block.is_open() {
            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index: GEMINI_THOUGHT_BLOCK_INDEX,
                content_block: OutputContentBlock::Thinking {
                    thinking: String::new(),
                    signature: None,
                },
            }));
            self.thought_block = GeminiBlockState::Open;
        }
        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: GEMINI_THOUGHT_BLOCK_INDEX,
            delta: ContentBlockDelta::ThinkingDelta {
                thinking: text.to_string(),
            },
        }));
    }

    fn message_start(&self) -> StreamEvent {
        StreamEvent::MessageStart(MessageStartEvent {
            message: MessageResponse {
                id: self.request_id.clone().unwrap_or_default(),
                kind: "message".to_string(),
                role: "assistant".to_string(),
                content: Vec::new(),
                model: self.model.clone(),
                stop_reason: None,
                stop_sequence: None,
                usage: Usage {
                    input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    output_tokens: 0,
                    output_tokens_details: None,
                },
                request_id: self.request_id.clone(),
                // Unknown at start; the signature rides on the closing delta.
                thought_signature: None,
                reasoning_replay: None,
                context_management: None,
            },
        })
    }

    fn ingest(&mut self, value: &Value) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        // The Code Assist backend wraps each chunk in `{"response": {...}}`; the
        // public Gemini surface would send the bare object, so accept either.
        let response = value.get("response").unwrap_or(value);
        if self.request_id.is_none() {
            self.request_id = response
                .get("responseId")
                .and_then(Value::as_str)
                .or_else(|| value.get("traceId").and_then(Value::as_str))
                .map(str::to_string);
        }
        if self.message_start.is_pending() {
            events.push(self.message_start());
            self.message_start = GeminiMessageStartState::Started;
        }
        let Some(candidate) = response
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
        else {
            return events;
        };
        if let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        // Gemini thought summaries (enabled via
                        // `thinkingConfig.includeThoughts`) arrive as text parts
                        // flagged `"thought": true`, ahead of the answer. Route
                        // them to a separate reasoning block so the TUI shows the
                        // model thinking during the wait instead of a blank
                        // screen, and so reasoning never splices into the answer.
                        if part.get("thought").and_then(Value::as_bool) == Some(true) {
                            self.push_thought_delta(&mut events, text);
                        } else {
                            self.push_text_delta(&mut events, text);
                        }
                    }
                }
                if let Some(call) = part.get("functionCall") {
                    self.visible_output = VisibleOutputState::Started;
                    // Gemini delivers a functionCall as a complete part. Close any
                    // open prose/reasoning block before surfacing the tool row so the
                    // reveal buffer does not later receive a lone empty stop event.
                    if self.thought_block.is_open() {
                        events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                            index: GEMINI_THOUGHT_BLOCK_INDEX,
                        }));
                        self.thought_block = GeminiBlockState::Closed;
                    }
                    if self.text_block.is_open() {
                        events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                            index: self.text_block_index,
                        }));
                        self.text_block = GeminiBlockState::Closed;
                    }

                    let index = self.next_content_index;
                    self.next_content_index += 1;
                    let name = call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("function")
                        .to_string();
                    let id = gemini_tool_call_id(call, self.tool_call_fallback_scope, index);
                    let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
                    events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                        index,
                        content_block: OutputContentBlock::ToolUse {
                            id,
                            name,
                            // Seed the finished args on the start event and do not emit an
                            // extra input_json_delta. That keeps Pending→Running layout
                            // stable and avoids duplicating JSON in downstream collectors.
                            input: args,
                        },
                    }));
                    events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                        index,
                    }));
                    self.fc_signatures.push(
                        part.get("thoughtSignature")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    );
                }
            }
        }
        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.stop_reason = Some(normalize_finish_reason(reason));
        }
        if response.get("usageMetadata").is_some() {
            self.usage = usage_from_response(response);
        }
        events
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if self.message_start.is_pending() {
            events.push(self.message_start());
            self.message_start = GeminiMessageStartState::Started;
        }
        // Close the thought block first (it opens before the answer), then the
        // answer block, so both reasoning and text finalize cleanly.
        if self.thought_block.is_open() {
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index: GEMINI_THOUGHT_BLOCK_INDEX,
            }));
            self.thought_block = GeminiBlockState::Closed;
        }
        if self.text_block.is_open() {
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index: self.text_block_index,
            }));
            self.text_block = GeminiBlockState::Closed;
        }
        events.push(StreamEvent::MessageDelta(MessageDeltaEvent {
            delta: MessageDelta {
                stop_reason: self
                    .stop_reason
                    .clone()
                    .or_else(|| Some("end_turn".to_string())),
                stop_sequence: None,
                thought_signature: encode_thought_signatures(&self.fc_signatures),
                reasoning_replay: None,
            },
            usage: self.usage,
            context_management: None,
        }));
        events.push(StreamEvent::MessageStop(MessageStopEvent {}));
        events
    }
}

/// Run the Code Assist setup check after login. Returns the project selected by
/// the server or env, if one is needed/available.
pub async fn setup_saved_user() -> Result<Option<String>, ApiError> {
    tokio::time::timeout(LOGIN_SETUP_TOTAL_TIMEOUT, setup_saved_user_inner())
        .await
        .map_err(|_| {
            ApiError::Auth(format!(
                "Google Gemini Code Assist setup timed out after {}s; token was saved",
                LOGIN_SETUP_TOTAL_TIMEOUT.as_secs()
            ))
        })?
}

async fn setup_saved_user_inner() -> Result<Option<String>, ApiError> {
    let Some(tokens) = load_fresh_oauth() else {
        return Err(ApiError::Auth(
            "Google Gemini OAuth token not found; run `/login google` first".into(),
        ));
    };
    GeminiCodeAssistClient::from_oauth(&tokens)
        .setup_user()
        .await
}

async fn setup_user_from_load_response(
    load: &Value,
    project_id: Option<String>,
    client: &GeminiCodeAssistClient,
) -> Result<Option<String>, ApiError> {
    if !load.is_object() {
        return Err(ApiError::Auth(
            "Gemini Code Assist loadCodeAssist returned an empty response".into(),
        ));
    }

    if load.get("currentTier").is_some_and(|tier| !tier.is_null()) {
        if let Some(project) = load
            .get("cloudaicompanionProject")
            .and_then(Value::as_str)
            .filter(|project| !project.is_empty())
        {
            return Ok(Some(project.to_string()));
        }
        if project_id.is_some() {
            return Ok(project_id);
        }
        return ineligible_or_project_required(load);
    }

    let Some(tier) = default_allowed_tier(load) else {
        return ineligible_or_project_required(load);
    };
    let tier_id = tier
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("legacy-tier");
    if tier_id != "free-tier" && project_id.is_none() {
        return ineligible_or_project_required(load);
    }

    let onboard_request = json!({
        "tierId": tier_id,
        "cloudaicompanionProject": if tier_id == "free-tier" { None } else { project_id.as_deref() },
        "metadata": client_metadata(project_id.as_deref()),
    });
    let mut operation = client
        .post_json_with_timeout::<Value>("onboardUser", &onboard_request, Some(LOGIN_HTTP_TIMEOUT))
        .await?;
    for _ in 0..SETUP_MAX_POLLS {
        if operation
            .get("done")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
        let Some(name) = operation.get("name").and_then(Value::as_str) else {
            break;
        };
        tokio::time::sleep(SETUP_POLL_INTERVAL).await;
        operation = client
            .get_json_with_timeout::<Value>(operation_url(name), Some(LOGIN_HTTP_TIMEOUT))
            .await?;
    }

    if let Some(project) = operation
        .pointer("/response/cloudaicompanionProject/id")
        .and_then(Value::as_str)
        .filter(|project| !project.is_empty())
    {
        return Ok(Some(project.to_string()));
    }
    if project_id.is_some() {
        return Ok(project_id);
    }
    ineligible_or_project_required(load)
}

fn default_allowed_tier(load: &Value) -> Option<&Value> {
    let tiers = load.get("allowedTiers")?.as_array()?;
    tiers
        .iter()
        .find(|tier| {
            tier.get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .or_else(|| tiers.first())
}

fn ineligible_or_project_required<T>(load: &Value) -> Result<T, ApiError> {
    if let Some(reasons) = load.get("ineligibleTiers").and_then(Value::as_array) {
        if !reasons.is_empty() {
            let summary = reasons
                .iter()
                .map(|reason| {
                    let tier = reason
                        .get("tierName")
                        .or_else(|| reason.get("tierId"))
                        .and_then(Value::as_str)
                        .unwrap_or("tier");
                    let message = reason
                        .get("reasonMessage")
                        .or_else(|| reason.get("validationErrorMessage"))
                        .and_then(Value::as_str)
                        .unwrap_or("ineligible");
                    format!("{tier}: {message}")
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ApiError::Auth(format!(
                "Gemini Code Assist account is not eligible: {summary}"
            )));
        }
    }
    Err(ApiError::Auth(
        "This Google account requires setting GOOGLE_CLOUD_PROJECT or GOOGLE_CLOUD_PROJECT_ID for Gemini Code Assist".into(),
    ))
}

fn google_cloud_project() -> Result<Option<String>, ApiError> {
    let project = std::env::var("GOOGLE_CLOUD_PROJECT")
        .ok()
        .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT_ID").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(project) = &project {
        if project.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(ApiError::Auth(format!(
                "Invalid Google Cloud Project ID: \"{project}\". Set GOOGLE_CLOUD_PROJECT to the string project ID, not the numeric project number."
            )));
        }
    }
    Ok(project)
}

fn client_metadata(project_id: Option<&str>) -> Value {
    let mut metadata = json!({
        "ideType": "ANTIGRAVITY",
        "platform": client_platform(),
        "pluginType": "GEMINI",
    });
    if let Some(project) = project_id.filter(|project| !project.is_empty()) {
        metadata["duetProject"] = Value::String(project.to_string());
    }
    metadata
}

/// The Cloud Code `ClientMetadata.Platform` proto enum value for the current
/// host. The backend validates this field strictly — a free-form label like
/// `"MACOS"` 400s with `Invalid value at 'metadata.platform'` — so it must be
/// one of the `{OS}_{ARCH}` enum variants, falling back to the always-valid
/// `PLATFORM_UNSPECIFIED`.
fn client_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "DARWIN_ARM64",
        ("macos", "x86_64") => "DARWIN_AMD64",
        ("linux", "aarch64") => "LINUX_ARM64",
        ("linux", "x86_64") => "LINUX_AMD64",
        ("windows", _) => "WINDOWS_AMD64",
        _ => "PLATFORM_UNSPECIFIED",
    }
}

/// Apply the Antigravity IDE identity headers that every Cloud Code request
/// carries. The backend gates personal/AI-Pro access on this metadata now that
/// the Gemini CLI Code Assist app is being retired, so the User-Agent,
/// `X-Goog-Api-Client`, and `Client-Metadata` must look like the IDE.
fn antigravity_headers(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header("User-Agent", antigravity_user_agent())
        .header("X-Goog-Api-Client", ANTIGRAVITY_API_CLIENT)
        .header("Client-Metadata", client_metadata(None).to_string())
}

fn antigravity_user_agent() -> String {
    let version = std::env::var(ANTIGRAVITY_VERSION_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| newest_version(installed_antigravity_version(), ANTIGRAVITY_VERSION));
    format!("antigravity/{version} darwin/arm64")
}

/// The newer of an installed IDE's version and the shipped floor, compared
/// numerically per dotted component — `2.10.0` is newer than `2.9.1`, and a
/// version the IDE reports below the floor must not pull the client below
/// what the Antigravity CLI presents.
fn newest_version(installed: Option<String>, floor: &str) -> String {
    let parts = |version: &str| -> Vec<u64> {
        version
            .split('.')
            .map(|part| part.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    match installed {
        Some(found) if parts(&found) > parts(floor) => found,
        _ => floor.to_string(),
    }
}

/// The version of the Antigravity IDE installed on this machine, read from its
/// bundle. Following the installed IDE keeps "new models appear on their own"
/// true when the IDE moves past the shipped floor; see [`ANTIGRAVITY_VERSION`]
/// for why the registry cares, and `newest_version` for why an older IDE does
/// not lower the client. The environment override still wins over both.
fn installed_antigravity_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![std::path::PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(std::path::PathBuf::from(home).join("Applications"));
        }
        roots.into_iter().find_map(|root| {
            let plist = root.join("Antigravity.app/Contents/Info.plist");
            let text = std::fs::read_to_string(plist).ok()?;
            let (_, after_key) = text.split_once("<key>CFBundleShortVersionString</key>")?;
            let (_, after_open) = after_key.split_once("<string>")?;
            let (version, _) = after_open.split_once("</string>")?;
            let version = version.trim();
            let plausible = !version.is_empty()
                && version
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.');
            plausible.then(|| version.to_string())
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn build_generate_content_request(request: &MessageRequest, project_id: Option<&str>) -> Value {
    // The ids and names every functionCall / functionResponse carries on the
    // wire, unique within the request (see `WireToolIds`). Built once over the
    // whole message list, then threaded into `message_to_content`.
    let wire_ids = WireToolIds::over(&request.messages);
    let contents = merge_consecutive_content_roles(
        request
            .messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| message_to_content(message, index, &wire_ids))
            .collect::<Vec<_>>(),
    );
    let mut inner = json!({
        "contents": contents,
        "generationConfig": {
            "maxOutputTokens": request.max_tokens,
        },
    });
    if let Some(system) = system_instruction(request) {
        inner["systemInstruction"] = system;
    }
    if let Some(tools) = request.tools.as_ref().filter(|tools| !tools.is_empty()) {
        inner["tools"] = json!([{
            "functionDeclarations": tools.iter().map(function_declaration).collect::<Vec<_>>(),
        }]);
    }
    if let Some(tool_choice) = request.tool_choice.as_ref() {
        inner["toolConfig"] = tool_config(tool_choice);
    }
    let (wire_model, thinking_level) = gemini_wire(&request.model, request.reasoning_request());
    // Gemini 3 controls reasoning depth through a string thinkingLevel derived
    // from the request effort (it dropped 2.5's numeric thinkingBudget).
    // `includeThoughts` asks the backend to stream human-readable thought
    // summaries (parts flagged `"thought": true`), which the decoder surfaces as
    // reasoning so the wait shows the model thinking instead of a blank screen.
    inner["generationConfig"]["thinkingConfig"] = json!({
        "thinkingLevel": thinking_level,
        "includeThoughts": true,
    });

    let mut payload = json!({
        "model": wire_model,
        "userAgent": "antigravity",
        "requestId": new_request_id(),
        "request": inner,
    });
    if let Some(project) = project_id.filter(|project| !project.is_empty()) {
        payload["project"] = Value::String(project.to_string());
    }
    payload
}

/// The Gemini 3 `thinkingLevel` for a request that resolved to `wire`.
///
/// Only `medium` needs the resolved id to decide: a model that publishes no
/// middle rung (Pro is `low`/`high` only) has its medium request resolved up to
/// the high id by the catalog, and sending `thinkingLevel: "medium"` alongside a
/// high-tier id would name a level that model does not expose. Comparing the
/// resolved ids answers that from the catalog data instead of from a hardcoded
/// list of which families tier — so a model added purely in settings gets the
/// right level with no code change.
fn gemini_thinking_level(model: &str, effort: EffortLevel, wire: &str) -> &'static str {
    match effort {
        EffortLevel::Low => "low",
        EffortLevel::Medium => {
            let high = super::wire_model_for_effort(model, EffortLevel::High)
                .unwrap_or_else(|| model.to_string());
            if high == wire { "high" } else { "medium" }
        }
        EffortLevel::High | EffortLevel::Xhigh | EffortLevel::Max | EffortLevel::Ultra => "high",
    }
}

/// Resolve a Zo model id plus the request effort into the Cloud Code wire model
/// id and its Gemini 3 `thinkingLevel`.
///
/// The mapping itself is data, not code: it lives in the shared model catalog
/// (`model_context_windows.json`, overridable through
/// `ZO_MODEL_CONTEXT_WINDOWS`, which is how a settings-declared model reaches
/// this crate). That matters because Google's serving ids bake the reasoning
/// tier in — the backend registry publishes `gemini-3.6-flash-low|-medium|-high`
/// and no bare `gemini-3.6-flash` — and a new release has historically arrived
/// as a new set of those ids. Teaching zo about one is a catalog edit.
///
/// A model the catalog says nothing about is sent under its own id. That is the
/// only honest default: deriving a variant from the id's shape would turn an
/// unknown model into a 404 (or, worse, silently serve a different model than
/// the one selected, which is what the previous version-collapsing heuristic
/// did — every `gemini-<n>.<m>-flash` became `gemini-3-flash`).
fn gemini_wire(model: &str, reasoning: ReasoningRequest) -> (String, &'static str) {
    let trimmed = model.trim();
    let effort = super::effort_rung(reasoning);
    let wire =
        super::wire_model_for_effort(trimmed, effort).unwrap_or_else(|| trimmed.to_string());
    let level = gemini_thinking_level(trimmed, effort, &wire);
    (wire, level)
}

fn system_instruction(request: &MessageRequest) -> Option<Value> {
    let text = request
        .system
        .as_ref()?
        .iter()
        .map(|block| match block {
            SystemBlock::Text { text, .. } => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    // Correct zo's hardcoded Claude identity for the Gemini-served model via
    // the shared override (Gemini has no system role, so the override rides in
    // the leading `role:"user"` content). Empty system stays empty.
    let text = super::apply_non_anthropic_identity(
        &text,
        &request.model,
        super::maker_for_provider(super::ProviderKind::Google),
    );
    (!text.is_empty()).then(|| json!({ "role": "user", "parts": [{ "text": text }] }))
}

/// Collapse consecutive same-role contents into one so the request keeps the
/// strict user/model alternation Gemini Code Assist enforces.
///
/// Gemini requires every `functionResponse` for a tool-call turn to live in the
/// single `user` content that immediately follows the `model` turn holding the
/// matching `functionCall`s. Zo stores each tool result as its own message
/// (`Session::push_message` is called once per tool result), so a *parallel*
/// tool call — N `functionCall`s emitted in one assistant turn — converts to N
/// separate single-`functionResponse` user contents. Gemini then rejects the
/// request with "Please ensure that the number of function response parts is
/// equal to the number of function call parts of the function call turn"
/// (400 `INVALID_ARGUMENT`). Anthropic and the OpenAI-compatible backends tolerate
/// the split, but Gemini does not. Merging adjacent same-role contents folds the
/// N responses back into one user turn, restoring call/response parity. Single
/// tool calls are unaffected (their lone result is already adjacent and alone).
fn merge_consecutive_content_roles(contents: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(contents.len());
    for content in contents {
        if let Some(last) = merged.last_mut() {
            if last.get("role") == content.get("role") {
                if let (Some(into), Some(from)) = (
                    last.get_mut("parts").and_then(Value::as_array_mut),
                    content.get("parts").and_then(Value::as_array),
                ) {
                    into.extend(from.iter().cloned());
                    continue;
                }
            }
        }
        merged.push(content);
    }
    merged
}

/// The tool-call ids a Gemini request carries — unique within the request — and
/// the declared function name each one answers to.
///
/// Gemini pairs a `functionResponse` to its `functionCall` by `id` plus `name`
/// (the declared function name; the official Gemini CLI sends `{ id: callId,
/// name: toolName, response }`). Zo's `ToolResult` blocks carry only the
/// `tool_use_id`, so the name is recovered from the call that result answers.
///
/// Gemini mints `functionCall.id` itself and does not keep it unique across a
/// conversation: on 2026-09-11 it issued `call_18194` to a `bash` call and, 36
/// turns later, to a `read_file` call. Replaying both made every following
/// request a bare `400 INVALID_ARGUMENT` — "Request contains an invalid
/// argument.", no details — and that session could not continue on Gemini; the
/// 09-06 session died the same way on `call_1147386`. So the history keeps the
/// provider's ids and the wire gives every repeat its own: the first occurrence
/// keeps its id, a later call with the same id gets `<id>-<n>` (never an id the
/// request already holds), and each result carries the wire id of the call it
/// answers by [`ToolCallPairing`]'s rule — the one the orphan seal pairs by, so
/// the sealed view reaches the wire whole: the next unanswered call with its
/// `tool_use_id` in the newest message that called it (results follow the call
/// they answer; one response repeating an id is answered in call order). A
/// result no waiting call takes (one too many) carries the latest such call's
/// wire id; one whose id no call carries (an orphan/replayed result) keeps its
/// own id and falls back to it as its name downstream.
struct WireToolIds {
    /// `(message index, block index)` of a `ToolUse` or `ToolResult` → its wire id.
    ids: HashMap<(usize, usize), String>,
    /// Wire id → the declared function name of that call.
    names: HashMap<String, String>,
}

impl WireToolIds {
    fn over(messages: &[InputMessage]) -> Self {
        let mut taken: std::collections::HashSet<String> = messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| match block {
                InputContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        let mut repeats: HashMap<&str, usize> = HashMap::new();
        let mut latest: HashMap<&str, String> = HashMap::new();
        // The wire id of every call occurrence, numbered for the pairing.
        let mut call_wire_ids: Vec<String> = Vec::new();
        let mut pairing = ToolCallPairing::new();
        let mut ids = HashMap::new();
        let mut names = HashMap::new();
        for (message_index, message) in messages.iter().enumerate() {
            for (block_index, block) in message.content.iter().enumerate() {
                match block {
                    InputContentBlock::ToolUse { id, name, .. } => {
                        let seen = repeats.entry(id.as_str()).or_insert(0);
                        let wire = if *seen == 0 {
                            id.clone()
                        } else {
                            let mut suffix = *seen;
                            loop {
                                let candidate = format!("{id}-{suffix}");
                                if !taken.contains(&candidate) {
                                    break candidate;
                                }
                                suffix += 1;
                            }
                        };
                        *seen += 1;
                        taken.insert(wire.clone());
                        names.insert(wire.clone(), name.clone());
                        latest.insert(id.as_str(), wire.clone());
                        pairing.call(message_index, id, call_wire_ids.len());
                        call_wire_ids.push(wire.clone());
                        ids.insert((message_index, block_index), wire);
                    }
                    InputContentBlock::ToolResult { tool_use_id, .. } => {
                        let wire = pairing
                            .result(tool_use_id)
                            .map(|occurrence| call_wire_ids[occurrence].clone())
                            .or_else(|| latest.get(tool_use_id.as_str()).cloned())
                            .unwrap_or_else(|| tool_use_id.clone());
                        ids.insert((message_index, block_index), wire);
                    }
                    _ => {}
                }
            }
        }
        Self { ids, names }
    }

    fn id_at(&self, message_index: usize, block_index: usize) -> Option<&str> {
        self.ids.get(&(message_index, block_index)).map(String::as_str)
    }

    fn name_of(&self, wire_id: &str) -> Option<&str> {
        self.names.get(wire_id).map(String::as_str)
    }
}

fn message_to_content(
    message: &InputMessage,
    message_index: usize,
    wire_ids: &WireToolIds,
) -> Option<Value> {
    let role = if message.role == "assistant" {
        "model"
    } else {
        "user"
    };
    let mut parts = message
        .content
        .iter()
        .enumerate()
        .filter_map(|(block_index, block)| {
            part_from_block(block, wire_ids.id_at(message_index, block_index), wire_ids)
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    // Echo each functionCall part's own thought signature back. Gemini 3 requires
    // the signature it emitted to be present on the *matching* functionCall in the
    // next request (per-part, in order), or multi-turn tool calls 400 — and for a
    // parallel call that means position 2 too, not just the first. Only this
    // (Gemini) encoder reads `InputMessage::thought_signature` — the Anthropic and
    // OpenAI encoders never look at it — so a signature minted by Gemini can never
    // leak into a request to another provider.
    if let Some(stored) = message.thought_signature.as_deref() {
        distribute_thought_signatures(stored, &mut parts);
    }
    // Any functionCall part still without a `thoughtSignature` would 400 the
    // request ("Function call is missing a thought_signature in functionCall
    // parts"). This happens for tool calls that entered history from a *different*
    // model (a mid-conversation swap to Gemini, whose calls were never signed) and
    // for the unsigned trailing parts Gemini itself leaves on a parallel call.
    // Google's documented escape hatch is to send the sentinel
    // `skip_thought_signature_validator`, which disables validation for that part
    // (mirroring LiteLLM's model-switch handling) instead of hard-failing the turn.
    backfill_missing_thought_signatures(&mut parts);
    Some(json!({ "role": role, "parts": parts }))
}

/// Google's documented sentinel that tells the Gemini API to skip
/// `thought_signature` validation for a `functionCall` part. Sent for calls that
/// carry no real signature (foreign-model history after a mid-conversation swap,
/// or the unsigned tail of a parallel call) so the request is accepted instead of
/// 400ing. See <https://ai.google.dev/gemini-api/docs/thought-signatures>.
const SKIP_THOUGHT_SIGNATURE_SENTINEL: &str = "skip_thought_signature_validator";

/// Stamp the skip sentinel onto every `functionCall` part that
/// [`distribute_thought_signatures`] did not already sign. Idempotent: parts that
/// already carry a real `thoughtSignature` are left untouched, so genuine
/// reasoning continuity is preserved and only truly-unsigned calls are exempted.
fn backfill_missing_thought_signatures(parts: &mut [Value]) {
    for part in parts.iter_mut() {
        let Some(object) = part.as_object_mut() else {
            continue;
        };
        if !object.contains_key("functionCall") {
            continue;
        }
        let missing = object
            .get("thoughtSignature")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty);
        if missing {
            object.insert(
                "thoughtSignature".to_string(),
                Value::String(SKIP_THOUGHT_SIGNATURE_SENTINEL.to_string()),
            );
        }
    }
}

/// Pack the per-functionCall-part thought signatures captured from a Gemini
/// response into the single opaque `thought_signature` slot that travels through
/// session storage and the cross-provider message types.
///
/// A response with at most one functionCall is stored verbatim as that part's
/// signature (or `None`) — backward-compatible with sessions persisted before
/// parallel-call support, and with the single-call wire the other readers expect.
/// Two or more functionCall parts are stored as a JSON array aligned to
/// functionCall order, with `null` for the parts the model left unsigned, so
/// [`distribute_thought_signatures`] can re-attach each to its own part. Returns
/// `None` when no part carried a signature, so a signature-free turn stores
/// nothing.
fn encode_thought_signatures(fc_signatures: &[Option<String>]) -> Option<String> {
    if !fc_signatures.iter().any(Option::is_some) {
        return None;
    }
    if fc_signatures.len() <= 1 {
        return fc_signatures.first().cloned().flatten();
    }
    let array = fc_signatures
        .iter()
        .map(|signature| signature.clone().map_or(Value::Null, Value::String))
        .collect::<Vec<_>>();
    Some(Value::Array(array).to_string())
}

/// Re-attach stored thought signatures (see [`encode_thought_signatures`]) onto
/// the request's functionCall parts, in order. A JSON-array payload distributes
/// each non-null entry onto the matching functionCall part; any other string is a
/// lone signature and lands on the first functionCall part (the legacy shape).
/// A Gemini signature is base64url and never begins with `[`, so the array form
/// is unambiguous.
fn distribute_thought_signatures(stored: &str, parts: &mut [Value]) {
    let function_call_indices: Vec<usize> = parts
        .iter()
        .enumerate()
        .filter(|(_, part)| part.get("functionCall").is_some())
        .map(|(index, _)| index)
        .collect();
    let set = |part: &mut Value, signature: &str| {
        if let Some(object) = part.as_object_mut() {
            object.insert(
                "thoughtSignature".to_string(),
                Value::String(signature.to_string()),
            );
        }
    };
    if stored.starts_with('[') {
        if let Ok(Value::Array(signatures)) = serde_json::from_str::<Value>(stored) {
            for (signature, &part_index) in signatures.iter().zip(&function_call_indices) {
                if let Some(value) = signature.as_str() {
                    set(&mut parts[part_index], value);
                }
            }
            return;
        }
    }
    if let Some(&first) = function_call_indices.first() {
        set(&mut parts[first], stored);
    }
}

fn part_from_block(
    block: &InputContentBlock,
    wire_id: Option<&str>,
    wire_ids: &WireToolIds,
) -> Option<Value> {
    match block {
        InputContentBlock::Text { text, .. } => Some(json!({ "text": text })),
        InputContentBlock::Image { source, .. } => Some(inline_data_part(source)),
        InputContentBlock::ToolUse { .. } => match ToolLedgerView::from_input_block(block) {
            Some(ToolLedgerView::ToolUse { id, name, input }) => Some(json!({
                "functionCall": {
                    "id": wire_id.unwrap_or(id),
                    "name": name,
                    "args": input,
                }
            })),
            _ => None,
        },
        InputContentBlock::ToolResult { .. } => match ToolLedgerView::from_input_block(block) {
            Some(ToolLedgerView::ToolResult {
                tool_use_id,
                content,
                is_error,
            }) => Some(json!({
                // Pair the response to its call the way Gemini matches them:
                // `id` is the call id and `name` is the *declared function name*
                // (resolved from the matching functionCall, falling back to the
                // id for an orphan result), mirroring the official Gemini CLI's
                // `{ id, name, response }`. Sending the id as the name made the
                // model treat the result as missing and re-issue the tool until
                // the sub-agent hit its iteration cap (Gemini-only runaway).
                "functionResponse": {
                    "id": wire_id.unwrap_or(tool_use_id),
                    "name": wire_ids
                        .name_of(wire_id.unwrap_or(tool_use_id))
                        .unwrap_or(tool_use_id),
                    "response": {
                        "content": super::flatten_tool_result_content(content),
                        "is_error": is_error,
                    }
                }
            })),
            _ => None,
        },
        // Anthropic reasoning blocks never cross into a Gemini request: thinking
        // signatures are provider-specific and would be meaningless (or rejected)
        // here. Dropped, mirroring the `thought_signature` isolation.
        InputContentBlock::Document { .. }
        | InputContentBlock::Thinking { .. }
        | InputContentBlock::RedactedThinking { .. } => None,
    }
}

fn inline_data_part(source: &ImageSource) -> Value {
    json!({
        "inlineData": {
            "mimeType": source.media_type,
            "data": source.data,
        }
    })
}

fn function_declaration(tool: &ToolDefinition) -> Value {
    let mut declaration = json!({
        "name": tool.name,
        "description": tool.description,
    });
    // A tool with no arguments — many MCP tools — declares NO `parameters`:
    // an OBJECT schema with empty `properties` is one of Gemini's known 400s
    // ("properties: should be non-empty for OBJECT type"), and a declaration
    // without parameters is its documented shape for a nullary function.
    let parameters = gemini_parameter_schema(&tool.input_schema);
    if !is_object_without_properties(&parameters) {
        declaration["parameters"] = parameters;
    }
    declaration
}

/// An OBJECT schema that names no properties — a nullary tool's `parameters`,
/// or a free-form `object` argument.
fn is_object_without_properties(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("object")
        && schema
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(serde_json::Map::is_empty)
}

/// Translate a tool's JSON Schema into the shape Gemini Code Assist accepts.
///
/// The backend parses function parameters as a protobuf `Schema`, whose `type`
/// is a single value rather than a JSON Schema type union. Passing an array such
/// as `"type": ["string", "boolean", "number"]` makes the proto JSON parser
/// reject the entire request ("Proto field is not repeating, cannot start
/// list"). This rewrites every type union into the equivalent Gemini form,
/// recursing through nested object/array schemas so unions at any depth (e.g. a
/// property's `type`) are handled:
/// - `["string", "null"]` -> `{ "type": "string", "nullable": true }`
/// - `["string", "number"]` -> `{ "anyOf": [{ "type": "string" }, { "type": "number" }] }`
///
/// JSON Schema metadata keywords such as `$schema` are also stripped because
/// Gemini's protobuf `Schema` rejects unknown fields before it can consider the
/// tool declaration.
fn gemini_parameter_schema(schema: &Value) -> Value {
    gemini_parameter_schema_inner(schema, false)
}

fn gemini_parameter_schema_inner(schema: &Value, properties_map: bool) -> Value {
    match schema {
        Value::Object(fields) => {
            let mut out = serde_json::Map::with_capacity(fields.len());
            for (key, value) in fields {
                if properties_map {
                    // Inside `properties`, keys are user-visible argument names,
                    // not Schema field names. Preserve them verbatim while still
                    // translating each property's schema value.
                    out.insert(key.clone(), gemini_parameter_schema_inner(value, false));
                    continue;
                }
                if is_unsupported_json_schema_keyword(key) {
                    continue;
                }
                match (key.as_str(), value) {
                    ("type", Value::Array(members)) => rewrite_type_union(members, &mut out),
                    // Gemini knows a handful of formats per type (STRING:
                    // `enum`/`date-time`, NUMBER: `float`/`double`, INTEGER:
                    // `int32`/`int64`); `uri`, `email`, `uuid` and the rest —
                    // routine in MCP tool schemas — are unknown values that 400
                    // the request. Dropped: the description still says what the
                    // value is.
                    ("format", Value::String(format)) => {
                        if is_gemini_format(fields.get("type").and_then(Value::as_str), format) {
                            out.insert(key.clone(), value.clone());
                        }
                    }
                    // Gemini's `enum` is string-only: an integer enum (`[1, 2, 3]`)
                    // is rejected, so it is dropped and the field stays as its
                    // type, unconstrained.
                    ("enum", Value::Array(members))
                        if !members.iter().all(Value::is_string) => {}
                    // Tuple validation (`items: [..]`) has no Gemini form; the
                    // first schema stands for every element.
                    ("items", Value::Array(members)) => {
                        if let Some(first) = members.first() {
                            out.insert(
                                key.clone(),
                                gemini_parameter_schema_inner(first, false),
                            );
                        }
                    }
                    // A nested OBJECT that names no properties keeps only its
                    // type: an empty `properties` map is the same 400 as at the
                    // top level.
                    ("properties", Value::Object(map)) if map.is_empty() => {}
                    // Gemini's proto `Schema` has no `oneOf`; it only understands
                    // `anyOf`. The exactly-one vs at-least-one distinction is not
                    // enforceable on the model anyway, so widen it to `anyOf`
                    // (sanitizing the branch schemas) instead of 400ing on an
                    // unknown field.
                    ("oneOf", Value::Array(members)) => {
                        out.insert(
                            "anyOf".to_string(),
                            Value::Array(
                                members
                                    .iter()
                                    .map(|member| gemini_parameter_schema_inner(member, false))
                                    .collect(),
                            ),
                        );
                    }
                    // Gemini has no `const`; a string literal is expressed as a
                    // single-value string `enum` (its only enum form). Non-string
                    // consts have no equivalent and are dropped.
                    ("const", literal) => rewrite_const(literal, &mut out),
                    // draft 2020-12 numeric exclusive bounds are unknown fields to
                    // Gemini; fold them into the inclusive bound so the constraint
                    // is not silently lost. The draft-04 boolean form (and any
                    // other shape) is dropped.
                    ("exclusiveMinimum", Value::Number(bound)) => {
                        out.entry("minimum".to_string())
                            .or_insert_with(|| Value::Number(bound.clone()));
                    }
                    ("exclusiveMaximum", Value::Number(bound)) => {
                        out.entry("maximum".to_string())
                            .or_insert_with(|| Value::Number(bound.clone()));
                    }
                    ("exclusiveMinimum" | "exclusiveMaximum", _) => {}
                    _ => {
                        out.insert(
                            key.clone(),
                            gemini_parameter_schema_inner(value, key == "properties"),
                        );
                    }
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| gemini_parameter_schema_inner(item, false))
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

/// JSON Schema keywords Gemini Code Assist's protobuf `Schema` has no field for
/// and cannot express. Passing any of them makes the backend reject the entire
/// tool declaration ("Unknown name ... Cannot find field"), so they are dropped.
/// Keywords with a lossless-enough Gemini equivalent (`type` unions, `oneOf`,
/// `const`, numeric `exclusiveMinimum`/`exclusiveMaximum`) are rewritten in
/// `gemini_parameter_schema_inner` instead of being listed here. Metadata
/// keywords (`$schema`, `$id`, `$ref`, `$comment`, ...) are covered by the
/// leading `$` check.
fn is_unsupported_json_schema_keyword(key: &str) -> bool {
    key.starts_with('$')
        || matches!(
            key,
            "definitions"
                | "additionalItems"
                | "uniqueItems"
                | "contentMediaType"
                | "contentEncoding"
                | "deprecated"
                | "propertyNames"
                | "patternProperties"
                | "unevaluatedProperties"
                | "unevaluatedItems"
                | "dependencies"
                | "dependentSchemas"
                | "dependentRequired"
                | "if"
                | "then"
                | "else"
                | "not"
                | "allOf"
                | "contains"
                | "minContains"
                | "maxContains"
                | "multipleOf"
                | "readOnly"
                | "writeOnly"
                | "examples"
        )
}

/// Whether `format` is one Gemini's `Schema` accepts for the given `type`
/// (<https://ai.google.dev/api/caching#Schema>): everything else is an
/// unknown value the backend rejects.
fn is_gemini_format(schema_type: Option<&str>, format: &str) -> bool {
    match schema_type {
        Some("string") => matches!(format, "enum" | "date-time"),
        Some("number") => matches!(format, "float" | "double"),
        Some("integer") => matches!(format, "int32" | "int64"),
        _ => false,
    }
}

/// Rewrite a JSON Schema `const` into the closest Gemini form. Gemini's `enum`
/// is string-only and must pair with `type: string`, so a string literal becomes
/// a one-value enum; any non-string const has no equivalent and is dropped (the
/// field stays, just unconstrained) rather than 400ing the request.
fn rewrite_const(literal: &Value, out: &mut serde_json::Map<String, Value>) {
    if let Value::String(literal) = literal {
        out.entry("type".to_string())
            .or_insert_with(|| Value::String("string".to_string()));
        out.insert(
            "enum".to_string(),
            Value::Array(vec![Value::String(literal.clone())]),
        );
    }
}

/// Expand a JSON Schema `type` union into Gemini's single-`type` + `nullable`
/// representation, falling back to `anyOf` for a genuine multi-type union so the
/// schema keeps its meaning instead of collapsing to one arbitrary type.
fn rewrite_type_union(members: &[Value], out: &mut serde_json::Map<String, Value>) {
    let mut concrete: Vec<String> = Vec::new();
    let mut nullable = false;
    for member in members.iter().filter_map(Value::as_str) {
        if member == "null" {
            nullable = true;
        } else if !concrete.iter().any(|existing| existing == member) {
            concrete.push(member.to_string());
        }
    }
    match concrete.as_slice() {
        [] => {}
        [single] => {
            out.insert("type".into(), Value::String(single.clone()));
        }
        _ => {
            let variants = concrete
                .into_iter()
                .map(|name| json!({ "type": name }))
                .collect::<Vec<_>>();
            out.insert("anyOf".into(), Value::Array(variants));
        }
    }
    if nullable {
        out.insert("nullable".into(), Value::Bool(true));
    }
}

fn tool_config(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!({ "functionCallingConfig": { "mode": "AUTO" } }),
        ToolChoice::Any => json!({ "functionCallingConfig": { "mode": "ANY" } }),
        ToolChoice::None => json!({ "functionCallingConfig": { "mode": "NONE" } }),
        ToolChoice::Tool { name } => json!({
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": [name],
            }
        }),
    }
}

fn normalize_generate_content_response(
    request: &MessageRequest,
    payload: &Value,
) -> Result<MessageResponse, ApiError> {
    let trace_id = payload
        .get("traceId")
        .and_then(Value::as_str)
        .unwrap_or("gemini-code-assist")
        .to_string();
    let response = payload.get("response").unwrap_or(&Value::Null);
    let candidate = response
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .ok_or(ApiError::InvalidSseFrame(
            "Gemini Code Assist response missing candidates",
        ))?;
    let mut content = Vec::new();
    let tool_call_fallback_scope = next_gemini_tool_call_fallback_scope();
    // One slot per functionCall part, in emission order. Gemini 3 attaches a
    // `thoughtSignature` (sibling of `functionCall` at the part level) to the
    // first one or two parts of a parallel call — not just the first — and the
    // backend 400s on the follow-up turn if a part that *did* carry one comes
    // back without it. Capturing per-part (not "first only") is what preserves
    // position 2's signature.
    let mut fc_signatures: Vec<Option<String>> = Vec::new();
    if let Some(parts) = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
    {
        for (index, part) in parts.iter().enumerate() {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    // A thought summary (`"thought": true`) becomes a reasoning
                    // block, not answer text, mirroring the streaming decoder so
                    // both paths render reasoning the same way.
                    if part.get("thought").and_then(Value::as_bool) == Some(true) {
                        content.push(OutputContentBlock::Thinking {
                            thinking: text.to_string(),
                            signature: None,
                        });
                    } else {
                        content.push(OutputContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }
            }
            if let Some(call) = part.get("functionCall") {
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("function")
                    .to_string();
                let fallback_index = u32::try_from(index).unwrap_or(u32::MAX);
                let id = gemini_tool_call_id(call, tool_call_fallback_scope, fallback_index);
                let input = call.get("args").cloned().unwrap_or_else(|| json!({}));
                content.push(OutputContentBlock::ToolUse { id, name, input });
                fc_signatures.push(
                    part.get("thoughtSignature")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                );
            }
        }
    }
    let thought_signature = encode_thought_signatures(&fc_signatures);

    Ok(MessageResponse {
        id: trace_id.clone(),
        kind: "message".to_string(),
        role: "assistant".to_string(),
        content,
        model: response
            .get("modelVersion")
            .and_then(Value::as_str)
            .unwrap_or(&request.model)
            .to_string(),
        stop_reason: candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .map(normalize_finish_reason),
        stop_sequence: None,
        usage: usage_from_response(response),
        request_id: Some(trace_id),
        thought_signature,
        reasoning_replay: None,
        context_management: None,
    })
}

fn usage_from_response(response: &Value) -> Usage {
    let usage = response.get("usageMetadata");
    // Gemini reports `promptTokenCount` INCLUDING its cached-content subset
    // (`cachedContentTokenCount`), and — separately from `candidatesTokenCount` —
    // the reasoning/thinking tokens in `thoughtsTokenCount`, which are billed as
    // output. Mirror the OpenAI-compat mapping (see `chatgpt_backend`): keep input
    // and cache-read DISJOINT so `Usage::total_tokens` does not double-count, and
    // fold thinking tokens into output so cost/context accounting isn't undercounted
    // on thinking models (previously thoughts + cache reads were silently dropped).
    let prompt = usage_field(usage, "promptTokenCount");
    let cache_read = usage_field(usage, "cachedContentTokenCount").min(prompt);
    Usage {
        input_tokens: prompt.saturating_sub(cache_read),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read,
        output_tokens: usage_field(usage, "candidatesTokenCount")
            .saturating_add(usage_field(usage, "thoughtsTokenCount")),
        output_tokens_details: None,
    }
}

fn usage_field(usage: Option<&Value>, field: &str) -> u32 {
    usage
        .and_then(|usage| usage.get(field))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

fn normalize_finish_reason(reason: &str) -> String {
    match reason {
        "STOP" => "end_turn",
        "MAX_TOKENS" => "max_tokens",
        "SAFETY" => "safety",
        "RECITATION" => "recitation",
        other => other,
    }
    .to_string()
}

/// Set to dump the last request body and the last error body to
/// `/tmp/zo-gemini-{request,error}.txt` — the way to see WHICH argument a
/// `400 INVALID_ARGUMENT` found invalid when the message alone does not say.
const GEMINI_DEBUG_ENV: &str = "ZO_GEMINI_DEBUG";

async fn api_error_from_response(
    status: reqwest::StatusCode,
    response: reqwest::Response,
) -> ApiError {
    let body = response.text().await.unwrap_or_default();
    super::debug_dump(
        GEMINI_DEBUG_ENV,
        "gemini",
        "error",
        &format!("status {status}\n{body}"),
    );
    ApiError::Api {
        status,
        error_type: google_error_type(&body),
        message: google_error_message(&body),
        body,
        retryable: is_retryable_status(status),
        retry_after: None,
    }
}

fn request_not_found_context(error: ApiError, requested_model: &str) -> ApiError {
    let ApiError::Api {
        status,
        error_type,
        message,
        body,
        retryable,
        retry_after,
    } = error
    else {
        return error;
    };
    if status != reqwest::StatusCode::NOT_FOUND {
        return ApiError::Api {
            status,
            error_type,
            message,
            body,
            retryable,
            retry_after,
        };
    }

    let provider_message = message.as_deref().unwrap_or("resource was not found");
    ApiError::Api {
        status,
        error_type,
        message: Some(format!(
            "Google Code Assist OAuth returned HTTP 404 while requesting model `{}` ({provider_message}). HTTP 404 alone does not prove that the model ID is unavailable: verify model access for this OAuth account and the configured Google project, endpoint, and API version. A public Gemini API model may also be unavailable through Code Assist OAuth.",
            requested_model.trim()
        )),
        body,
        retryable,
        retry_after,
    }
}

fn google_error_type(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body).ok().and_then(|value| {
        value
            .pointer("/error/status")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn google_error_message(body: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(body).ok()?;
    let message = value
        .pointer("/error/message")
        .or_else(|| value.get("error_description"))
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)?;
    let details = google_error_details(&value);
    Some(if details.is_empty() {
        message.to_string()
    } else {
        format!("{message} — {details}")
    })
}

/// The `details[]` a Google error carries beside its one-line message — a
/// `BadRequest`'s field violations, an `ErrorInfo`'s reason and domain — as one
/// line, so a `400 INVALID_ARGUMENT` says WHICH argument was invalid instead of
/// only that one was ("Request contains an invalid argument.", 2026-09-06).
fn google_error_details(value: &Value) -> String {
    let Some(details) = value.pointer("/error/details").and_then(Value::as_array) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for detail in details {
        if let Some(violations) = detail.get("fieldViolations").and_then(Value::as_array) {
            for violation in violations {
                let field = violation.get("field").and_then(Value::as_str).unwrap_or("?");
                let description = violation
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                parts.push(if description.is_empty() {
                    field.to_string()
                } else {
                    format!("{field}: {description}")
                });
            }
        } else if let Some(reason) = detail.get("reason").and_then(Value::as_str) {
            match detail.get("domain").and_then(Value::as_str) {
                Some(domain) => parts.push(format!("{reason} ({domain})")),
                None => parts.push(reason.to_string()),
            }
        }
    }
    parts.join("; ")
}

const fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    let code = status.as_u16();
    matches!(code, 408 | 409 | 429 | 499) || code >= 500
}

pub(crate) fn method_url(method: &str) -> String {
    format!("{}:{method}", base_url())
}

fn operation_url(name: &str) -> String {
    format!("{}/{}", base_url(), name)
}

fn base_url() -> String {
    let endpoint = std::env::var(CODE_ASSIST_ENDPOINT_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CODE_ASSIST_ENDPOINT.to_string());
    let version = std::env::var(CODE_ASSIST_API_VERSION_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CODE_ASSIST_API_VERSION.to_string());
    format!("{}/{}", endpoint.trim_end_matches('/'), version)
}

fn new_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("zo-{nanos}")
}

/// Re-stamp the top-level `requestId` of a serialized generateContent payload
/// for a restart re-send, leaving every other field semantically unchanged
/// (the JSON round-trips through `serde_json::Value`, so key order/whitespace
/// may differ — object semantics, not raw bytes, are the contract here).
/// Falls back to the original bytes if the payload is not the JSON object
/// this module itself serialized (defensive only — it always is).
fn body_with_fresh_request_id(body: &[u8]) -> Vec<u8> {
    let Ok(mut payload) = serde_json::from_slice::<Value>(body) else {
        return body.to_vec();
    };
    let Some(object) = payload.as_object_mut() else {
        return body.to_vec();
    };
    object.insert("requestId".to_string(), Value::String(new_request_id()));
    serde_json::to_vec(&payload).unwrap_or_else(|_| body.to_vec())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
