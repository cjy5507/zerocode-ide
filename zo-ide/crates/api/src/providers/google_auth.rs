//! Google access-token minting shared by Google-backed providers.
//!
//! Application Default Credentials (ADC) are the durable OAuth seam used by
//! both the Claude-on-Vertex gateway and Gemini's OpenAI-compatible adapter:
//!
//! - `service_account` (`GOOGLE_APPLICATION_CREDENTIALS`): a self-signed
//!   RS256 JWT (signed with `ring`, already in the dependency tree via rustls)
//!   exchanged at the account's `token_uri` for a one-hour access token.
//! - `authorized_user` (the `gcloud auth application-default login` file): a
//!   refresh-token grant, optionally narrowed to the scopes requested by the
//!   caller.
//! - Fallback to the appropriate `gcloud` access-token command when ADC cannot
//!   be parsed locally.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::sync_bridge::lock_recovered;
use core_types::{OAuthAuthorizationRequest, OAuthConfig, PkceCodePair};

/// OAuth scope every Google Cloud/Vertex call needs.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
/// Gemini OAuth quickstart scope for the Generative Language API. Google
/// documents ADC setup with this plus `cloud-platform`; keep the exact value
/// centralized so login, token minting, and tests cannot drift.
const GENERATIVE_LANGUAGE_RETRIEVER_SCOPE: &str =
    "https://www.googleapis.com/auth/generative-language.retriever";
/// Direct pre-minted access-token override for Gemini/Google adapter requests.
pub const GOOGLE_ACCESS_TOKEN_ENV: &str = "GOOGLE_ACCESS_TOKEN";
/// Optional path to the OAuth desktop-client JSON. Passed to `gcloud` when the
/// CLI is available; used directly by Zo's built-in browser OAuth fallback
/// when `gcloud` is not installed.
pub const GOOGLE_OAUTH_CLIENT_ID_FILE_ENV: &str = "GOOGLE_OAUTH_CLIENT_ID_FILE";
/// Google access tokens last ~60 minutes; refresh comfortably before expiry.
const ACCESS_TOKEN_TTL: Duration = Duration::from_secs(45 * 60);
/// Token endpoint for `authorized_user` refresh grants (service accounts
/// carry their own `token_uri` in the credentials file).
const USER_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

const VERTEX_SCOPES: &[&str] = &[CLOUD_PLATFORM_SCOPE];
const GEMINI_SCOPES: &[&str] = &[CLOUD_PLATFORM_SCOPE, GENERATIVE_LANGUAGE_RETRIEVER_SCOPE];

/// OAuth client metadata from a Google "Desktop app" client-secret JSON.
///
/// Zo uses this only when the Google Cloud CLI is unavailable: it performs
/// the same browser OAuth flow itself, then writes an ADC `authorized_user`
/// file that the normal Gemini token path already understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleOAuthClientConfig {
    pub client_id: String,
    pub client_secret: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub project_id: Option<String>,
}

impl GoogleOAuthClientConfig {
    fn oauth_config(&self) -> OAuthConfig {
        OAuthConfig {
            client_id: self.client_id.clone(),
            authorize_url: self.auth_uri.clone(),
            token_url: self.token_uri.clone(),
            callback_port: None,
            manual_redirect_url: None,
            scopes: GEMINI_SCOPES
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
            client_secret: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedGoogleAdc {
    pub path: PathBuf,
    pub access_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GoogleOAuthTokenResponse {
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdcCredentials {
    ServiceAccount {
        client_email: String,
        private_key_pem: String,
        token_uri: String,
    },
    AuthorizedUser {
        client_id: String,
        client_secret: String,
        refresh_token: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GcloudTokenCommand {
    /// `gcloud auth print-access-token` — preserves the existing Vertex
    /// fallback, which may be backed by user or service-account gcloud auth.
    UserAuth,
    /// `gcloud auth application-default print-access-token` — matches the
    /// Gemini OAuth quickstart and the ADC file we parse directly.
    ApplicationDefault,
}

impl GcloudTokenCommand {
    const fn args(self) -> &'static [&'static str] {
        match self {
            Self::UserAuth => &["auth", "print-access-token"],
            Self::ApplicationDefault => &["auth", "application-default", "print-access-token"],
        }
    }
}

struct TokenCacheEntry {
    key: String,
    token: String,
    minted_at: Instant,
}

static ACCESS_TOKEN_CACHE: Mutex<Vec<TokenCacheEntry>> = Mutex::new(Vec::new());

/// Comma-separated scope argument for `gcloud auth application-default login`.
#[must_use]
pub fn gemini_oauth_scopes_csv() -> String {
    GEMINI_SCOPES.join(",")
}

/// Load a Google OAuth client JSON downloaded from Google Cloud Console.
/// Supports the common `installed` desktop-app shape and `web` shape so users
/// can point `GOOGLE_OAUTH_CLIENT_ID_FILE` at either client-secret file.
pub fn load_google_oauth_client_config(
    path: impl AsRef<Path>,
) -> Result<GoogleOAuthClientConfig, ApiError> {
    let path = path.as_ref();
    let contents = std::fs::read_to_string(path).map_err(ApiError::Io)?;
    parse_google_oauth_client_config(&contents).map_err(|message| {
        ApiError::Auth(format!(
            "failed to parse Google OAuth client file {}: {message}",
            path.display()
        ))
    })
}

/// Parse a Google OAuth client-secret JSON body.
pub fn parse_google_oauth_client_config(contents: &str) -> Result<GoogleOAuthClientConfig, String> {
    let value: Value = serde_json::from_str(contents).map_err(|error| error.to_string())?;
    let client = value
        .get("installed")
        .or_else(|| value.get("web"))
        .ok_or_else(|| "expected top-level `installed` or `web` object".to_string())?;
    let field = |name: &str| {
        client
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("missing `{name}`"))
    };
    Ok(GoogleOAuthClientConfig {
        client_id: field("client_id")?,
        client_secret: field("client_secret")?,
        auth_uri: field("auth_uri")?,
        token_uri: field("token_uri")?,
        project_id: client
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

/// Build the Zo-managed browser OAuth URL used when `gcloud` is absent.
#[must_use]
pub fn google_oauth_authorize_url(
    client: &GoogleOAuthClientConfig,
    redirect_uri: &str,
    state: &str,
    pkce: &PkceCodePair,
) -> String {
    OAuthAuthorizationRequest::from_config(&client.oauth_config(), redirect_uri, state, pkce)
        .with_extra_param("access_type", "offline")
        .with_extra_param("prompt", "consent")
        .build_url()
}

/// Complete a Zo-managed Google OAuth code exchange and persist a gcloud-
/// compatible ADC `authorized_user` file. The returned access token is cached
/// for this process; the refresh token in the ADC file is used after restart.
pub async fn exchange_google_oauth_code_and_save_adc(
    client: &GoogleOAuthClientConfig,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<SavedGoogleAdc, ApiError> {
    let token = exchange_google_oauth_code(client, code, code_verifier, redirect_uri).await?;
    let path = save_authorized_user_adc(client, &token.refresh_token)?;
    cache_token(
        format!("gemini:{}", GEMINI_SCOPES.join(" ")),
        token.access_token.clone(),
    );
    Ok(SavedGoogleAdc {
        path,
        access_token: token.access_token,
    })
}

/// Network-free Gemini OAuth/ADC availability probe for provider enablement and
/// `/connect`: env access-token override or a readable ADC file candidate.
#[must_use]
pub fn gemini_oauth_available() -> bool {
    env_non_empty(GOOGLE_ACCESS_TOKEN_ENV)
        || (!crate::managed_account::external_credentials_disabled()
            && adc_credentials_path(&|key| std::env::var(key).ok())
                .is_some_and(|path| path.is_file()))
}

/// Resolve a Gemini access token from `GOOGLE_ACCESS_TOKEN`, ADC, or gcloud.
pub async fn gemini_access_token() -> Result<String, ApiError> {
    access_token_for_scopes(
        "gemini",
        GEMINI_SCOPES,
        Some(GOOGLE_ACCESS_TOKEN_ENV),
        GcloudTokenCommand::ApplicationDefault,
    )
    .await
}

/// Resolve a Vertex access token while preserving its existing env override and
/// `gcloud auth print-access-token` fallback semantics.
pub(crate) async fn vertex_access_token() -> Result<String, ApiError> {
    access_token_for_scopes(
        "vertex",
        VERTEX_SCOPES,
        Some("ANTHROPIC_VERTEX_ACCESS_TOKEN"),
        GcloudTokenCommand::UserAuth,
    )
    .await
}

async fn access_token_for_scopes(
    cache_namespace: &str,
    scopes: &'static [&'static str],
    env_override: Option<&str>,
    gcloud_command: GcloudTokenCommand,
) -> Result<String, ApiError> {
    if let Some(env_key) = env_override {
        if let Ok(token) = std::env::var(env_key).map(|value| value.trim().to_string()) {
            if !token.is_empty() {
                return Ok(token);
            }
        }
    }

    let cache_key = format!("{cache_namespace}:{}", scopes.join(" "));
    if let Some(token) = cached_token(&cache_key) {
        return Ok(token);
    }

    let adc_error = match adc_access_token(scopes).await {
        Ok(Some(token)) => {
            cache_token(cache_key, token.clone());
            return Ok(token);
        }
        Ok(None) => None,
        Err(error) => Some(error),
    };
    if let Some(error) = &adc_error {
        eprintln!("zo: Google ADC token mint failed, falling back to gcloud: {error}");
    }

    match gcloud_access_token(gcloud_command).await {
        Ok(token) => {
            cache_token(cache_key, token.clone());
            Ok(token)
        }
        Err(gcloud_error) => {
            let adc_suffix = adc_error
                .map(|error| format!(" ADC failed first: {error}."))
                .unwrap_or_default();
            let recovery_hint = if cache_namespace == "vertex" {
                "Install the gcloud CLI and run `gcloud auth application-default login`, or set ANTHROPIC_VERTEX_ACCESS_TOKEN.".to_string()
            } else {
                format!(
                    "Run `gcloud auth application-default login --scopes={}` or set GOOGLE_API_KEY.",
                    gemini_oauth_scopes_csv()
                )
            };
            Err(ApiError::Auth(format!(
                "Google OAuth access token unavailable.{adc_suffix} {gcloud_error}. {recovery_hint}"
            )))
        }
    }
}

fn cached_token(cache_key: &str) -> Option<String> {
    lock_recovered(&ACCESS_TOKEN_CACHE)
        .iter()
        .find(|entry| entry.key == cache_key && entry.minted_at.elapsed() < ACCESS_TOKEN_TTL)
        .map(|entry| entry.token.clone())
}

fn cache_token(cache_key: String, token: String) {
    let mut cache = lock_recovered(&ACCESS_TOKEN_CACHE);
    if let Some(entry) = cache.iter_mut().find(|entry| entry.key == cache_key) {
        entry.token = token;
        entry.minted_at = Instant::now();
        return;
    }
    cache.push(TokenCacheEntry {
        key: cache_key,
        token,
        minted_at: Instant::now(),
    });
}

/// `GOOGLE_APPLICATION_CREDENTIALS`, else gcloud's well-known ADC path.
pub(crate) fn adc_credentials_path(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(explicit) = lookup("GOOGLE_APPLICATION_CREDENTIALS")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return Some(PathBuf::from(explicit));
    }
    lookup("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("gcloud")
            .join("application_default_credentials.json")
    })
}

/// Parse an ADC file body. `None` for unknown `type`s (e.g.
/// `external_account` workload identity — out of scope, gcloud fallback
/// handles those machines).
pub(crate) fn parse_adc(contents: &str) -> Option<AdcCredentials> {
    let value: Value = serde_json::from_str(contents).ok()?;
    let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    match value.get("type").and_then(Value::as_str)? {
        "service_account" => Some(AdcCredentials::ServiceAccount {
            client_email: field("client_email")?,
            private_key_pem: field("private_key")?,
            token_uri: field("token_uri").unwrap_or_else(|| USER_TOKEN_URI.to_string()),
        }),
        "authorized_user" => Some(AdcCredentials::AuthorizedUser {
            client_id: field("client_id")?,
            client_secret: field("client_secret")?,
            refresh_token: field("refresh_token")?,
        }),
        _ => None,
    }
}

/// Quota/billing project hint for OAuth requests. Google’s Gemini OAuth curl
/// examples send `x-goog-user-project`; prefer explicit env, then ADC metadata.
#[must_use]
pub(crate) fn request_user_project() -> Option<String> {
    [
        "GOOGLE_CLOUD_PROJECT",
        "GOOGLE_PROJECT_ID",
        "GCLOUD_PROJECT",
    ]
    .into_iter()
    .find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
    .or_else(|| {
        let path = adc_credentials_path(&|key| std::env::var(key).ok())?;
        let contents = read_private_credential_text(&path).ok()?;
        parse_adc_project_hint(&contents)
    })
}

fn read_private_credential_text(path: &Path) -> std::io::Result<String> {
    core_types::paths::restrict_permissions_owner_only(path)?;
    String::from_utf8(core_types::paths::read_private_file(path)?)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn parse_adc_project_hint(contents: &str) -> Option<String> {
    let value: Value = serde_json::from_str(contents).ok()?;
    ["quota_project_id", "project_id"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn adc_access_token(scopes: &'static [&'static str]) -> Result<Option<String>, String> {
    let Some(path) = adc_credentials_path(&|key| std::env::var(key).ok()) else {
        return Ok(None);
    };
    let contents = match read_private_credential_text(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read ADC file {}: {error}",
                path.display()
            ));
        }
    };
    let Some(credentials) = parse_adc(&contents) else {
        return Ok(None);
    };
    fetch_access_token(
        &crate::providers::shared_http_client(),
        &credentials,
        scopes,
    )
    .await
    .map(Some)
}

async fn gcloud_access_token(command: GcloudTokenCommand) -> Result<String, String> {
    let args = command.args();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("gcloud").args(args).output()
    })
    .await
    .map_err(|join_error| format!("gcloud token task failed: {join_error}"))?
    .map_err(|io_error| format!("failed to run `gcloud {}` ({io_error})", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "`gcloud {}` exited with {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(format!(
            "`gcloud {}` returned an empty access token",
            args.join(" ")
        ));
    }
    Ok(token)
}

async fn exchange_google_oauth_code(
    client: &GoogleOAuthClientConfig,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<GoogleOAuthTokenResponse, ApiError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client.client_id.as_str()),
        ("client_secret", client.client_secret.as_str()),
        ("code_verifier", code_verifier),
    ];
    let response = crate::providers::shared_http_client()
        .post(&client.token_uri)
        .timeout(std::time::Duration::from_secs(20))
        .form(&form)
        .send()
        .await
        .map_err(ApiError::Http)?;
    let status = response.status();
    let body: Value = response.json().await.map_err(ApiError::Http)?;
    if !status.is_success() {
        return Err(ApiError::Auth(format!(
            "Google OAuth token exchange failed ({status}): {}",
            body.get("error_description")
                .or_else(|| body.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        )));
    }
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ApiError::Auth("Google OAuth response carried no access_token".into()))?;
    let refresh_token = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            ApiError::Auth(
                "Google OAuth response carried no refresh_token; retry `/login google` so Zo can request offline access/consent"
                    .into(),
            )
        })?;
    Ok(GoogleOAuthTokenResponse {
        access_token,
        refresh_token,
    })
}

fn save_authorized_user_adc(
    client: &GoogleOAuthClientConfig,
    refresh_token: &str,
) -> Result<PathBuf, ApiError> {
    let path = default_adc_credentials_path().map_err(ApiError::Io)?;
    let mut value = json!({
        "type": "authorized_user",
        "client_id": &client.client_id,
        "client_secret": &client.client_secret,
        "refresh_token": refresh_token,
    });
    if let Some(project_id) = client.project_id.as_deref() {
        value["quota_project_id"] = Value::String(project_id.to_string());
    }
    let body = serde_json::to_vec_pretty(&value).map_err(ApiError::Json)?;
    // The file holds the long-lived `client_secret` and `refresh_token`, so it
    // must be written owner-only at creation with symlinks rejected. Reuse the
    // shared credential-write policy rather than duplicating the handling.
    core_types::paths::write_secret_file(&path, &body).map_err(ApiError::Io)?;
    Ok(path)
}

/// The well-known ADC file path written by `gcloud auth application-default login`.
pub fn default_adc_credentials_path() -> std::io::Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("HOME is not set; cannot locate Google ADC credentials path: {error}"),
        )
    })?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("gcloud")
        .join("application_default_credentials.json"))
}

/// Exchange ADC credentials for an access token.
pub(crate) async fn fetch_access_token(
    client: &reqwest::Client,
    credentials: &AdcCredentials,
    scopes: &'static [&'static str],
) -> Result<String, String> {
    let scope = scopes.join(" ");
    let (token_uri, form): (&str, Vec<(&str, String)>) = match credentials {
        AdcCredentials::ServiceAccount {
            client_email,
            private_key_pem,
            token_uri,
        } => {
            let issued_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let assertion =
                build_jwt_assertion(client_email, token_uri, issued_at, private_key_pem, scopes)?;
            (
                token_uri.as_str(),
                vec![
                    (
                        "grant_type",
                        "urn:ietf:params:oauth:grant-type:jwt-bearer".to_string(),
                    ),
                    ("assertion", assertion),
                ],
            )
        }
        AdcCredentials::AuthorizedUser {
            client_id,
            client_secret,
            refresh_token,
        } => {
            let mut form = vec![
                ("grant_type", "refresh_token".to_string()),
                ("client_id", client_id.clone()),
                ("client_secret", client_secret.clone()),
                ("refresh_token", refresh_token.clone()),
            ];
            if !scope.is_empty() {
                form.push(("scope", scope));
            }
            (USER_TOKEN_URI, form)
        }
    };
    let response = client
        .post(token_uri)
        .timeout(std::time::Duration::from_secs(15))
        .form(&form)
        .send()
        .await
        .map_err(|error| format!("Google token endpoint unreachable: {error}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("Google token endpoint returned non-JSON: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "Google token endpoint rejected the grant ({status}): {}",
            body.get("error_description")
                .or_else(|| body.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        ));
    }
    body.get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "Google token response carried no access_token".to_string())
}

/// `header.claims.signature` — the self-signed RS256 assertion a service
/// account trades for an access token. `issued_at` is a parameter so the
/// claim assembly is testable without freezing the clock.
pub(crate) fn build_jwt_assertion(
    client_email: &str,
    token_uri: &str,
    issued_at: u64,
    private_key_pem: &str,
    scopes: &'static [&'static str],
) -> Result<String, String> {
    let header = URL_SAFE_NO_PAD.encode(json!({"alg": "RS256", "typ": "JWT"}).to_string());
    let claims = URL_SAFE_NO_PAD.encode(
        json!({
            "iss": client_email,
            "scope": scopes.join(" "),
            "aud": token_uri,
            "iat": issued_at,
            "exp": issued_at + 3_600,
        })
        .to_string(),
    );
    let message = format!("{header}.{claims}");

    let der = pem_to_pkcs8_der(private_key_pem)?;
    let key_pair = ring::signature::RsaKeyPair::from_pkcs8(&der)
        .map_err(|error| format!("service-account private key rejected: {error}"))?;
    let mut signature = vec![0u8; key_pair.public().modulus_len()];
    key_pair
        .sign(
            &ring::signature::RSA_PKCS1_SHA256,
            &ring::rand::SystemRandom::new(),
            message.as_bytes(),
            &mut signature,
        )
        .map_err(|error| format!("RS256 signing failed: {error}"))?;
    Ok(format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

fn env_non_empty(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

/// Strip the `-----BEGIN/END PRIVATE KEY-----` armor and decode the body.
fn pem_to_pkcs8_der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .filter(|line| !line.contains("-----"))
        .map(str::trim)
        .collect();
    STANDARD
        .decode(body)
        .map_err(|error| format!("private key PEM is not valid base64: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 테스트 전용 throwaway 2048-bit 키 (실서비스 어디에도 안 쓰임).
    /// pii-scan: allow credential — 이 서명 시험을 위해 만든 일회용 키이며 어떤 계정도 열지 않는다.
    const TEST_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC1/Egcdzb6ZceW
4fHDjuxbQbX6Fzr9bS2cPhhfAgDZ+D53fNlGcOlLfRpU6AJxEtLz9S6ViCmQiCaY
NCtOKlJhWnILqAahgjUfPFLTok5Qxj069XOEhvEHfviR1raL5IL81jEEpsglOdT+
f8qfyqRupv8Wb5VVvn3w7d48nAYEX1oIxjuiWevCYh4GJUzIESgA19WBeucl6cAx
rUW0662jiRhT4ZyavuAIyk0BEM9fj5pB4Mm7nfPcnAIR2NBXM6+Pxwj1tyfP5qd+
fD0OWckknLTM1YQR3Sk4qfKx1/PARYCJ5aZe6x0yECjiD+8CRPGedfmUk38paFGA
msV6/BbFAgMBAAECggEAMicFhHrCNv2HpKg95WPk9T1FtldilWbaM/3U35IAxBEq
vek1Q7loQbqHYDDUQ28pnbvLC8CLm945rKZr7M2zCEtRtK6orSfiFeqc9N/87zvC
shXksPgzQpqWTDK8+g6Onrk0pxCDhebLMRvsrl69NBVnpTo5EHk/4f7byR5Cdj+M
mukKXvlQdG7c8o5TYjPpsMoxm8HAcnLdPPrLGJTGx1HcSZZoVi7mBTejAXD41pen
3Fv26ppVZuOpeI7C4G6zmoCmxAHDdrudLziIhoRPSd6fVQqPmzAvdWdH5KstPotB
V3xjb7iPU7Efmukg1oXsTeKqSfShbrRUd2pAzEuwvQKBgQD6agZXbJNAwofqSAeD
0KYsCWm+A/B70JYtce3cc/ouGIz0thbAceqzZm0P5pCc4md4GaY1hjjWxWquSJO0
vTYGyX9kKXOgJk+qRSr1wi/hKgib3fxkmdME+9rMHylLh4BytXAEwMx1HgzCms+4
HQJunzymC0JbDYYD3JcxAeVk9wKBgQC6C3+8LpR8OmpkBzVxTkmMgULwnH31D6Co
Kx3hi3HXI1WOMy2ym0gzjLJdnjUntWD3KqezvPQPbGvRraCC0LHtE7lYY8OhK0EQ
/8158PDi3AGMMCZvFsA+ClauF9Vler5WSsLW1xX7NMqy1ScEK8tApnrSoKYYezkT
QsTT7E2/IwKBgQDLoliR01tTuF2qaPSjfpMDEIyK1s1DAnZ9cj5JnY5+2bwWa9TI
nlqLlOlvmsFSstINWl5M/F9QV63PGHn06kD69/S+UO8T9tOl1SWAQG+LHRFvHu/W
JzjwvpZIk7aTExejMGRtmRMq0kryHc55HC4UIy3AoTtOrAqlLUdNtQsENQKBgETb
fKtpkgtok3fyMxV8pDwcm2nygavx3MRhMO4Jbljx+vhmeMNiNZbevCVqKMJJn1nb
r7YWeT48Iqu4V3ATTccxRagxRHaiS7K++o3nX0CXrPr110PGZ+COcwZ8S78Dbu8B
PJvHf5s6LsuBmK8yhkenVk4ep1roQHegfrjw/NWBAoGAER/dKopUByMw7kevsVBP
5qbz9vi8OlqLxjVtC9Bgxh/G8oNkqJQ4jVYriI2nYQeHOAg/QEMVgpe3mRZ3/cQg
HGpDZEQhKt2jyzfijuox8xgPx+DBKKehhPzzOaItL1FCYWKjiO0KYydEjEK262T8
7LdnfaEmHDP0ZUiqdvNKqHI=
-----END PRIVATE KEY-----";

    /// JWT 의 세 세그먼트: 헤더/클레임이 정확하고, 서명은 같은 키의 공개키로
    /// 검증된다 (ring 자체 검증 API 로 라운드트립).
    #[test]
    fn jwt_assertion_round_trips_signature_and_claims() {
        let assertion = build_jwt_assertion(
            "svc@proj.iam.gserviceaccount.com",
            "https://oauth2.googleapis.com/token",
            1_700_000_000,
            TEST_KEY_PEM,
            VERTEX_SCOPES,
        )
        .expect("assertion");
        let segments: Vec<&str> = assertion.split('.').collect();
        assert_eq!(segments.len(), 3);

        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segments[0]).expect("b64"))
                .expect("header json");
        assert_eq!(header["alg"], "RS256");
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segments[1]).expect("b64"))
                .expect("claims json");
        assert_eq!(claims["iss"], "svc@proj.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], CLOUD_PLATFORM_SCOPE);
        assert_eq!(claims["exp"], 1_700_003_600_u64);

        let der = pem_to_pkcs8_der(TEST_KEY_PEM).expect("der");
        let key_pair = ring::signature::RsaKeyPair::from_pkcs8(&der).expect("key");
        let message = format!("{}.{}", segments[0], segments[1]);
        let signature = URL_SAFE_NO_PAD.decode(segments[2]).expect("sig b64");
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            key_pair.public(),
        )
        .verify(message.as_bytes(), &signature)
        .expect("signature must verify against the same key's public half");
    }

    #[test]
    fn parses_google_oauth_client_secret_and_builds_offline_authorize_url() {
        let client = parse_google_oauth_client_config(
            r#"{
                "installed": {
                    "client_id": "cid.apps.googleusercontent.com",
                    "project_id": "proj-1",
                    "auth_uri": "https://accounts.google.com/o/oauth2/v2/auth",
                    "token_uri": "https://oauth2.googleapis.com/token",
                    "client_secret": "secret"
                }
            }"#,
        )
        .expect("client config");
        assert_eq!(client.client_id, "cid.apps.googleusercontent.com");
        assert_eq!(client.client_secret, "secret");
        assert_eq!(client.project_id.as_deref(), Some("proj-1"));

        let pkce = PkceCodePair {
            verifier: "verifier".to_string(),
            challenge: "challenge".to_string(),
            challenge_method: core_types::PkceChallengeMethod::S256,
        };
        let url = google_oauth_authorize_url(
            &client,
            "http://127.0.0.1:54545/callback",
            "state value",
            &pkce,
        );
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("client_id=cid.apps.googleusercontent.com"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A54545%2Fcallback"));
        assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcloud-platform%20https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fgenerative-language.retriever"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("code_challenge=challenge"));
    }

    #[test]
    fn rejects_google_oauth_client_secret_without_client_secret() {
        let error = parse_google_oauth_client_config(
            r#"{"installed":{"client_id":"cid","auth_uri":"https://auth","token_uri":"https://token"}}"#,
        )
        .expect_err("missing client_secret must be rejected");
        assert!(error.contains("client_secret"));
    }

    #[test]
    fn parses_both_adc_flavors_and_rejects_unknown() {
        let service_account = r#"{
            "type": "service_account",
            "client_email": "svc@proj.iam.gserviceaccount.com",
            "private_key": "-----BEGIN PRIVATE KEY-----\nxx\n-----END PRIVATE KEY-----",
            "token_uri": "https://oauth2.googleapis.com/token"
        }"#;
        assert!(matches!(
            parse_adc(service_account),
            Some(AdcCredentials::ServiceAccount { client_email, .. })
                if client_email == "svc@proj.iam.gserviceaccount.com"
        ));

        let authorized_user = r#"{
            "type": "authorized_user",
            "client_id": "cid",
            "client_secret": "csec",
            "refresh_token": "rtok"
        }"#;
        assert!(matches!(
            parse_adc(authorized_user),
            Some(AdcCredentials::AuthorizedUser { refresh_token, .. }) if refresh_token == "rtok"
        ));

        // Workload identity files fall through to the gcloud CLI path.
        assert_eq!(parse_adc(r#"{"type": "external_account"}"#), None);
        assert_eq!(parse_adc("not json"), None);
    }

    #[test]
    fn extracts_adc_project_hint_for_user_project_header() {
        assert_eq!(
            parse_adc_project_hint(
                r#"{"type":"authorized_user","quota_project_id":"quota-proj","project_id":"raw-proj"}"#
            )
            .as_deref(),
            Some("quota-proj")
        );
        assert_eq!(
            parse_adc_project_hint(r#"{"type":"service_account","project_id":"svc-proj"}"#)
                .as_deref(),
            Some("svc-proj")
        );
        assert_eq!(
            parse_adc_project_hint(r#"{"type":"authorized_user"}"#),
            None
        );
    }

    #[test]
    fn adc_path_prefers_explicit_env() {
        let lookup = |key: &str| match key {
            "GOOGLE_APPLICATION_CREDENTIALS" => Some("/tmp/sa.json".to_string()),
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        assert_eq!(
            adc_credentials_path(&lookup),
            Some(PathBuf::from("/tmp/sa.json"))
        );
        let lookup = |key: &str| (key == "HOME").then(|| "/home/u".to_string());
        assert_eq!(
            adc_credentials_path(&lookup),
            Some(PathBuf::from(
                "/home/u/.config/gcloud/application_default_credentials.json"
            ))
        );
    }

    #[cfg(unix)]
    fn sample_oauth_client() -> GoogleOAuthClientConfig {
        GoogleOAuthClientConfig {
            client_id: "cid.apps.googleusercontent.com".to_string(),
            client_secret: "top-secret".to_string(),
            auth_uri: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_uri: USER_TOKEN_URI.to_string(),
            project_id: Some("proj-1".to_string()),
        }
    }

    #[cfg(unix)]
    #[test]
    fn save_authorized_user_adc_is_owner_only_and_symlink_safe() {
        use std::os::unix::fs::PermissionsExt as _;

        let _lock = crate::test_env_lock();
        // Reuse the shared credential-store isolation: it points `HOME` (which
        // `default_adc_credentials_path` reads) at a fresh private temp dir and
        // restores it on drop, even on a panicking assertion.
        let isolation = crate::test_env::CredentialEnvIsolation::empty();

        let path = save_authorized_user_adc(&sample_oauth_client(), "refresh-xyz")
            .expect("adc save should succeed");
        assert!(
            path.starts_with(isolation.config_home()),
            "adc path must resolve under the isolated HOME"
        );

        let file_mode = std::fs::metadata(&path)
            .expect("adc file metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "adc file must be owner-only, got {file_mode:o}");

        let parent_mode = std::fs::metadata(path.parent().expect("adc parent"))
            .expect("adc parent metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            parent_mode, 0o700,
            "gcloud dir must be owner-only, got {parent_mode:o}"
        );

        // A rewrite over the existing regular file must still succeed and keep
        // the restrictive bits.
        let path2 = save_authorized_user_adc(&sample_oauth_client(), "refresh-abc")
            .expect("adc rewrite should succeed");
        assert_eq!(path, path2);
        let contents = std::fs::read_to_string(&path).expect("adc contents");
        assert!(contents.contains("refresh-abc"));
    }

    #[cfg(unix)]
    #[test]
    fn save_authorized_user_adc_rejects_symlinked_target() {
        let _lock = crate::test_env_lock();
        let isolation = crate::test_env::CredentialEnvIsolation::empty();

        let path = default_adc_credentials_path().expect("adc path");
        std::fs::create_dir_all(path.parent().expect("adc parent")).expect("create gcloud dir");
        let elsewhere = isolation.config_home().join("attacker-target.json");
        std::os::unix::fs::symlink(&elsewhere, &path).expect("plant symlink");

        let error = save_authorized_user_adc(&sample_oauth_client(), "refresh-xyz")
            .expect_err("writing through a symlink must fail");
        assert!(matches!(error, ApiError::Io(_)), "expected io error, got {error:?}");
        assert!(
            !elsewhere.exists(),
            "the symlink target must not have been written through"
        );
    }
}
