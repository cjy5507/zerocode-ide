//! OpenAI (ChatGPT) OAuth 2.0 PKCE authentication.
//!
//! Mirrors the method used by OpenAI's Codex CLI and the OpenCode
//! `opencode-openai-codex-auth` plugin: a ChatGPT Plus/Pro user signs in
//! through the browser, and the resulting `access_token` is sent straight to
//! the ChatGPT backend (`chatgpt.com/backend-api/codex`) — no API key, billed
//! against the ChatGPT subscription instead of API credits.
//!
//! This module owns the OAuth half only: building the authorize URL,
//! exchanging the authorization code, refreshing tokens, and pulling the
//! ChatGPT `account_id` out of the `id_token` JWT. The inference backend that
//! consumes the token lives separately.

use core_types::{OAuthAuthorizationRequest, OAuthConfig, OpenAiOAuthTokens, PkceCodePair};
use serde::Deserialize;

use crate::error::ApiError;

/// Public Codex CLI OAuth client id. Not a secret: it is a *public* client that
/// authenticates via PKCE. Same value used by OpenAI's Codex CLI and OpenCode.
pub const OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// OpenAI OAuth authorize endpoint.
pub const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
/// OpenAI OAuth token endpoint (authorization-code exchange + refresh).
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Loopback port the Codex OAuth client expects its `/auth/callback` on.
pub const OPENAI_OAUTH_CALLBACK_PORT: u16 = 1455;

/// The `id_token` JWT claim namespace holding ChatGPT account details.
const OPENAI_AUTH_CLAIM: &str = "https://api.openai.com/auth";

#[must_use]
fn openai_scopes() -> Vec<String> {
    ["openid", "profile", "email", "offline_access"]
        .iter()
        .map(|scope| (*scope).to_string())
        .collect()
}

/// Base OAuth client configuration for ChatGPT sign-in. Codex-specific
/// authorize-time query params are layered on in [`openai_authorize_url`] so
/// [`OAuthConfig`] itself stays provider-agnostic.
#[must_use]
pub fn openai_oauth_config() -> OAuthConfig {
    OAuthConfig {
        client_id: OPENAI_OAUTH_CLIENT_ID.to_string(),
        authorize_url: OPENAI_AUTHORIZE_URL.to_string(),
        token_url: OPENAI_TOKEN_URL.to_string(),
        callback_port: Some(OPENAI_OAUTH_CALLBACK_PORT),
        manual_redirect_url: None,
        scopes: openai_scopes(),
        client_secret: None,
    }
}

/// Build the browser authorization URL, adding the Codex-specific extra query
/// parameters the OpenAI authorize endpoint expects.
#[must_use]
pub fn openai_authorize_url(
    config: &OAuthConfig,
    redirect_uri: impl Into<String>,
    state: impl Into<String>,
    pkce: &PkceCodePair,
) -> String {
    OAuthAuthorizationRequest::from_config(config, redirect_uri, state, pkce)
        .with_extra_param("id_token_add_organizations", "true")
        .with_extra_param("codex_cli_simplified_flow", "true")
        .with_extra_param("originator", "codex_cli_rs")
        .build_url()
}

/// Standard OAuth 2.0 + OIDC token endpoint response.
#[derive(Debug, Deserialize)]
struct OpenAiTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

impl OpenAiTokenResponse {
    /// Normalise into the persisted bundle: `expires_in` (relative secs) →
    /// absolute `expires_at`, space-delimited `scope` → `scopes`, and
    /// `account_id` parsed from the `id_token` JWT. `fallback_refresh` carries
    /// the prior refresh token forward when a refresh response omits one.
    fn into_tokens(self, fallback_refresh: Option<String>) -> OpenAiOAuthTokens {
        let account_id = self.id_token.as_deref().and_then(account_id_from_id_token);
        let expires_at = self.expires_in.map(|secs| now_unix().saturating_add(secs));
        let scopes = self
            .scope
            .map(|scope| scope.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        OpenAiOAuthTokens {
            access_token: self.access_token,
            refresh_token: self.refresh_token.or(fallback_refresh),
            expires_at,
            account_id,
            scopes,
        }
    }
}

/// Exchange an authorization `code` for an access/refresh/id token set.
pub async fn exchange_openai_code(
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<OpenAiOAuthTokens, ApiError> {
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", OPENAI_OAUTH_CLIENT_ID),
        ("code_verifier", code_verifier),
    ];
    Ok(post_token_form(&params).await?.into_tokens(None))
}

/// Refresh an expired access token using a stored `refresh_token`.
pub async fn refresh_openai_tokens(refresh_token: &str) -> Result<OpenAiOAuthTokens, ApiError> {
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", OPENAI_OAUTH_CLIENT_ID),
    ];
    Ok(post_token_form(&params)
        .await?
        .into_tokens(Some(refresh_token.to_string())))
}

async fn post_token_form(params: &[(&str, &str)]) -> Result<OpenAiTokenResponse, ApiError> {
    let response = super::shared_http_client()
        .post(OPENAI_TOKEN_URL)
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
            message: None,
            body,
            retryable: false,
            retry_after: None,
        });
    }
    response
        .json::<OpenAiTokenResponse>()
        .await
        .map_err(ApiError::from)
}

/// Extract the ChatGPT `account_id` from an `id_token` JWT.
///
/// The signature is not verified — the token was just delivered to us over TLS
/// from the OAuth endpoint. Only the payload segment is decoded, reading
/// `["https://api.openai.com/auth"]["chatgpt_account_id"]`.
#[must_use]
pub fn account_id_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get(OPENAI_AUTH_CLAIM)?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_string)
}

/// Dependency-free base64url decoder (URL-safe alphabet, padding optional).
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    const fn sextet(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &byte in input.as_bytes() {
        if byte == b'=' {
            break;
        }
        acc = (acc << 6) | u32::from(sextet(byte)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            // Mask to the byte being emitted so the cast is provably in range.
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAiTokenResponse, account_id_from_id_token, base64url_decode, now_unix,
        openai_authorize_url, openai_oauth_config,
    };
    use core_types::{PkceChallengeMethod, PkceCodePair};

    /// Test-local base64url encoder (no padding) so we can synthesise JWTs.
    fn b64url(data: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        let mut index = 0;
        while index + 3 <= data.len() {
            let block = (u32::from(data[index]) << 16)
                | (u32::from(data[index + 1]) << 8)
                | u32::from(data[index + 2]);
            out.push(TABLE[((block >> 18) & 63) as usize] as char);
            out.push(TABLE[((block >> 12) & 63) as usize] as char);
            out.push(TABLE[((block >> 6) & 63) as usize] as char);
            out.push(TABLE[(block & 63) as usize] as char);
            index += 3;
        }
        match data.len() - index {
            1 => {
                let block = u32::from(data[index]) << 16;
                out.push(TABLE[((block >> 18) & 63) as usize] as char);
                out.push(TABLE[((block >> 12) & 63) as usize] as char);
            }
            2 => {
                let block = (u32::from(data[index]) << 16) | (u32::from(data[index + 1]) << 8);
                out.push(TABLE[((block >> 18) & 63) as usize] as char);
                out.push(TABLE[((block >> 12) & 63) as usize] as char);
                out.push(TABLE[((block >> 6) & 63) as usize] as char);
            }
            _ => {}
        }
        out
    }

    #[test]
    fn config_has_codex_client_and_endpoints() {
        let config = openai_oauth_config();
        assert_eq!(config.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
        assert_eq!(
            config.authorize_url,
            "https://auth.openai.com/oauth/authorize"
        );
        assert_eq!(config.token_url, "https://auth.openai.com/oauth/token");
        assert_eq!(config.callback_port, Some(1455));
        assert!(config.scopes.contains(&"offline_access".to_string()));
    }

    #[test]
    fn authorize_url_carries_codex_extra_params() {
        let config = openai_oauth_config();
        let pkce = PkceCodePair {
            verifier: "verifier".into(),
            challenge: "challenge".into(),
            challenge_method: PkceChallengeMethod::S256,
        };
        let url = openai_authorize_url(
            &config,
            "http://localhost:1455/auth/callback",
            "state-1",
            &pkce,
        );
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains("originator=codex_cli_rs"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
    }

    #[test]
    fn account_id_extracted_from_id_token_claim() {
        let payload =
            br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acc_test_123"},"sub":"u"}"#;
        let jwt = format!(
            "{}.{}.{}",
            b64url(b"{\"alg\":\"none\"}"),
            b64url(payload),
            "sig"
        );
        assert_eq!(
            account_id_from_id_token(&jwt).as_deref(),
            Some("acc_test_123")
        );
    }

    #[test]
    fn account_id_absent_when_claim_missing_or_malformed() {
        let jwt = format!("{}.{}.{}", b64url(b"{}"), b64url(br#"{"sub":"u"}"#), "sig");
        assert_eq!(account_id_from_id_token(&jwt), None);
        assert_eq!(account_id_from_id_token("not-a-jwt"), None);
    }

    #[test]
    fn base64url_round_trips() {
        for sample in [&b"hello"[..], &b"\x00\xff\x10"[..], &b"{}"[..]] {
            let encoded = b64url(sample);
            assert_eq!(base64url_decode(&encoded).as_deref(), Some(sample));
        }
    }

    #[test]
    fn token_response_resolves_expiry_scope_account() {
        let payload = br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acc9"}}"#;
        let jwt = format!("{}.{}.{}", b64url(b"{}"), b64url(payload), "s");
        let response = OpenAiTokenResponse {
            access_token: "at".into(),
            refresh_token: Some("rt".into()),
            id_token: Some(jwt),
            expires_in: Some(3600),
            scope: Some("openid profile".into()),
        };
        let before = now_unix();
        let tokens = response.into_tokens(None);
        assert_eq!(tokens.access_token, "at");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt"));
        assert_eq!(tokens.account_id.as_deref(), Some("acc9"));
        assert_eq!(
            tokens.scopes,
            vec!["openid".to_string(), "profile".to_string()]
        );
        let expires_at = tokens.expires_at.expect("expires_at resolved");
        assert!(expires_at >= before + 3600 && expires_at <= now_unix() + 3600);
    }

    #[test]
    fn refresh_keeps_existing_refresh_token_when_omitted() {
        let response = OpenAiTokenResponse {
            access_token: "new".into(),
            refresh_token: None,
            id_token: None,
            expires_in: Some(60),
            scope: None,
        };
        let tokens = response.into_tokens(Some("old_rt".into()));
        assert_eq!(tokens.refresh_token.as_deref(), Some("old_rt"));
    }
}
