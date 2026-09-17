//! OAuth data types shared across crates.
//!
//! These are pure data structures with no runtime or filesystem dependencies.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// OAuth client configuration used by the main Zo runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    pub client_id: String,
    pub authorize_url: String,
    pub token_url: String,
    pub callback_port: Option<u16>,
    pub manual_redirect_url: Option<String>,
    pub scopes: Vec<String>,
    /// Confidential-client secret for `token_endpoint_auth_method=client_secret_post`
    /// (obtained via Dynamic Client Registration). `None` for public PKCE clients,
    /// which is every provider except MCP servers that require DCR.
    pub client_secret: Option<String>,
}

/// Persisted OAuth access token bundle used by the CLI.
/// Note: credentials.json uses camelCase via `StoredOAuthCredentials` wrapper.
/// This struct uses `snake_case` for internal/API compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// OpenAI (ChatGPT) OAuth token bundle.
///
/// Unlike [`OAuthTokenSet`], this carries the ChatGPT `account_id` parsed from
/// the `id_token` JWT, which the ChatGPT backend requires in the
/// `chatgpt-account-id` request header. It is persisted under a separate
/// `openai_oauth` credentials key so it never collides with the Anthropic
/// `oauth` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiOAuthTokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    /// ChatGPT account id from the `id_token` JWT `https://api.openai.com/auth`
    /// claim; sent as the `chatgpt-account-id` header on backend requests.
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// PKCE verifier/challenge pair generated for an OAuth authorization flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceCodePair {
    pub verifier: String,
    pub challenge: String,
    pub challenge_method: PkceChallengeMethod,
}

/// Challenge algorithms supported by the local PKCE helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkceChallengeMethod {
    S256,
}

impl PkceChallengeMethod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::S256 => "S256",
        }
    }
}

/// Parameters needed to build an authorization URL for browser-based login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthAuthorizationRequest {
    pub authorize_url: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub state: String,
    pub code_challenge: String,
    pub code_challenge_method: PkceChallengeMethod,
    pub extra_params: BTreeMap<String, String>,
}

/// Request body for exchanging an OAuth authorization code for tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthTokenExchangeRequest {
    pub grant_type: &'static str,
    pub code: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub code_verifier: String,
    pub state: String,
    /// Sent in the POST body (`client_secret_post`) when the client is confidential.
    pub client_secret: Option<String>,
}

/// Request body for refreshing an existing OAuth token set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthRefreshRequest {
    pub grant_type: &'static str,
    pub refresh_token: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    /// Sent in the POST body (`client_secret_post`) when the client is confidential.
    pub client_secret: Option<String>,
}

/// Parsed query parameters returned to the local OAuth callback endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

impl OAuthAuthorizationRequest {
    #[must_use]
    pub fn from_config(
        config: &OAuthConfig,
        redirect_uri: impl Into<String>,
        state: impl Into<String>,
        pkce: &PkceCodePair,
    ) -> Self {
        Self {
            authorize_url: config.authorize_url.clone(),
            client_id: config.client_id.clone(),
            redirect_uri: redirect_uri.into(),
            scopes: config.scopes.clone(),
            state: state.into(),
            code_challenge: pkce.challenge.clone(),
            code_challenge_method: pkce.challenge_method,
            extra_params: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_extra_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_params.insert(key.into(), value.into());
        self
    }

    #[must_use]
    pub fn build_url(&self) -> String {
        let mut url = String::with_capacity(
            self.authorize_url.len()
                + self.client_id.len()
                + self.redirect_uri.len()
                + self.state.len()
                + self.code_challenge.len()
                + self.scopes.iter().map(String::len).sum::<usize>()
                + self.extra_params.len() * 16,
        );
        url.push_str(&self.authorize_url);
        url.push(if self.authorize_url.contains('?') {
            '&'
        } else {
            '?'
        });

        let mut first = true;
        append_query_param(&mut url, &mut first, "response_type", "code");
        append_query_param(&mut url, &mut first, "client_id", &self.client_id);
        append_query_param(&mut url, &mut first, "redirect_uri", &self.redirect_uri);
        append_query_param_joined(&mut url, &mut first, "scope", &self.scopes, " ");
        append_query_param(&mut url, &mut first, "state", &self.state);
        append_query_param(&mut url, &mut first, "code_challenge", &self.code_challenge);
        append_query_param(
            &mut url,
            &mut first,
            "code_challenge_method",
            self.code_challenge_method.as_str(),
        );
        for (key, value) in &self.extra_params {
            append_query_param(&mut url, &mut first, key, value);
        }

        url
    }
}

impl OAuthTokenExchangeRequest {
    #[must_use]
    pub fn from_config(
        config: &OAuthConfig,
        code: impl Into<String>,
        state: impl Into<String>,
        verifier: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        Self {
            grant_type: "authorization_code",
            code: code.into(),
            redirect_uri: redirect_uri.into(),
            client_id: config.client_id.clone(),
            code_verifier: verifier.into(),
            state: state.into(),
            client_secret: config.client_secret.clone(),
        }
    }

    #[must_use]
    pub fn form_params(&self) -> BTreeMap<&str, String> {
        let mut params = BTreeMap::from([
            ("grant_type", self.grant_type.to_string()),
            ("code", self.code.clone()),
            ("redirect_uri", self.redirect_uri.clone()),
            ("client_id", self.client_id.clone()),
            ("code_verifier", self.code_verifier.clone()),
            ("state", self.state.clone()),
        ]);
        if let Some(secret) = &self.client_secret {
            params.insert("client_secret", secret.clone());
        }
        params
    }
}

impl OAuthRefreshRequest {
    #[must_use]
    pub fn from_config(
        config: &OAuthConfig,
        refresh_token: impl Into<String>,
        scopes: Option<Vec<String>>,
    ) -> Self {
        Self {
            grant_type: "refresh_token",
            refresh_token: refresh_token.into(),
            client_id: config.client_id.clone(),
            scopes: scopes.unwrap_or_else(|| config.scopes.clone()),
            client_secret: config.client_secret.clone(),
        }
    }

    #[must_use]
    pub fn form_params(&self) -> BTreeMap<&str, String> {
        let mut params = BTreeMap::from([
            ("grant_type", self.grant_type.to_string()),
            ("refresh_token", self.refresh_token.clone()),
            ("client_id", self.client_id.clone()),
            ("scope", self.scopes.join(" ")),
        ]);
        if let Some(secret) = &self.client_secret {
            params.insert("client_secret", secret.clone());
        }
        params
    }
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(&mut encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

fn append_query_param(url: &mut String, first: &mut bool, key: &str, value: &str) {
    if !*first {
        url.push('&');
    }
    *first = false;
    url.push_str(&percent_encode(key));
    url.push('=');
    url.push_str(&percent_encode(value));
}

fn append_query_param_joined(
    url: &mut String,
    first: &mut bool,
    key: &str,
    values: &[String],
    separator: &str,
) {
    if !*first {
        url.push('&');
    }
    *first = false;
    url.push_str(&percent_encode(key));
    url.push('=');

    let mut values = values.iter();
    if let Some(value) = values.next() {
        url.push_str(&percent_encode(value));
        let encoded_separator = percent_encode(separator);
        for value in values {
            url.push_str(&encoded_separator);
            url.push_str(&percent_encode(value));
        }
    }
}

fn percent_decode(value: &str) -> Result<String, String> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hi = decode_hex(bytes[index + 1])?;
                let lo = decode_hex(bytes[index + 2])?;
                decoded.push((hi << 4) | lo);
                index += 3;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|error| error.to_string())
}

fn decode_hex(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("invalid percent byte: {byte}")),
    }
}

pub fn parse_oauth_callback_request_target(target: &str) -> Result<OAuthCallbackParams, String> {
    let (path, query) = target
        .split_once('?')
        .map_or((target, ""), |(path, query)| (path, query));
    if !path.ends_with("/callback")
        && !path.ends_with("/oauth2callback")
        && !path.ends_with("/oauth-callback")
    {
        return Err(format!("unexpected callback path: {path}"));
    }
    parse_oauth_callback_query(query)
}

pub fn parse_oauth_callback_query(query: &str) -> Result<OAuthCallbackParams, String> {
    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;

    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair
            .split_once('=')
            .map_or((pair, ""), |(key, value)| (key, value));

        let value = percent_decode(value)?;
        match percent_decode(key)?.as_str() {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "error" => error = Some(value),
            "error_description" => error_description = Some(value),
            _ => {}
        }
    }

    Ok(OAuthCallbackParams {
        code,
        state,
        error,
        error_description,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        OAuthAuthorizationRequest, OAuthCallbackParams, OAuthConfig, OAuthRefreshRequest,
        OAuthTokenExchangeRequest, PkceChallengeMethod, PkceCodePair, parse_oauth_callback_query,
        parse_oauth_callback_request_target,
    };

    fn secret_test_config(client_secret: Option<String>) -> OAuthConfig {
        OAuthConfig {
            client_id: "client".into(),
            authorize_url: "https://example.test/authorize".into(),
            token_url: "https://example.test/token".into(),
            callback_port: None,
            manual_redirect_url: None,
            scopes: vec!["read".into()],
            client_secret,
        }
    }

    #[test]
    fn token_exchange_includes_client_secret_when_present() {
        let config = secret_test_config(Some("s3cr3t".into()));
        let request = OAuthTokenExchangeRequest::from_config(
            &config,
            "code",
            "state",
            "verifier",
            "http://127.0.0.1:1/callback",
        );
        let params = request.form_params();
        assert_eq!(params.get("client_secret"), Some(&"s3cr3t".to_string()));
    }

    #[test]
    fn token_exchange_omits_client_secret_when_absent() {
        let config = secret_test_config(None);
        let request = OAuthTokenExchangeRequest::from_config(
            &config,
            "code",
            "state",
            "verifier",
            "http://127.0.0.1:1/callback",
        );
        let params = request.form_params();
        assert!(!params.contains_key("client_secret"));
    }

    #[test]
    fn refresh_includes_client_secret_when_present() {
        let config = secret_test_config(Some("s3cr3t".into()));
        let request = OAuthRefreshRequest::from_config(&config, "refresh", None);
        let params = request.form_params();
        assert_eq!(params.get("client_secret"), Some(&"s3cr3t".to_string()));
    }

    #[test]
    fn build_url_renders_required_and_extra_params() {
        let config = OAuthConfig {
            client_id: "client".into(),
            authorize_url: "https://example.test/oauth/authorize".into(),
            token_url: "https://example.test/oauth/token".into(),
            callback_port: Some(3000),
            manual_redirect_url: None,
            scopes: vec!["org:read".into(), "user:write".into()],
            client_secret: None,
        };
        let pkce = PkceCodePair {
            verifier: "verifier".into(),
            challenge: "challenge".into(),
            challenge_method: PkceChallengeMethod::S256,
        };

        let url = OAuthAuthorizationRequest::from_config(
            &config,
            "http://127.0.0.1:3000/callback",
            "state-123",
            &pkce,
        )
        .with_extra_param("prompt", "consent")
        .build_url();

        assert!(url.starts_with("https://example.test/oauth/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client"));
        assert!(url.contains("scope=org%3Aread%20user%3Awrite"));
        assert!(url.contains("prompt=consent"));
    }

    #[test]
    fn build_url_preserves_existing_query_and_exact_encoding() {
        let request = OAuthAuthorizationRequest {
            authorize_url: "https://example.test/oauth/authorize?existing=1".into(),
            client_id: "client id".into(),
            redirect_uri: "http://127.0.0.1:3000/callback".into(),
            scopes: vec!["org:read".into(), "user write".into()],
            state: "state/value".into(),
            code_challenge: "challenge".into(),
            code_challenge_method: PkceChallengeMethod::S256,
            extra_params: [("audience".into(), "claude code".into())]
                .into_iter()
                .collect(),
        };

        assert_eq!(
            request.build_url(),
            "https://example.test/oauth/authorize?existing=1&response_type=code&client_id=client%20id&redirect_uri=http%3A%2F%2F127.0.0.1%3A3000%2Fcallback&scope=org%3Aread%20user%20write&state=state%2Fvalue&code_challenge=challenge&code_challenge_method=S256&audience=claude%20code"
        );
    }

    #[test]
    fn parse_callback_query_decodes_percent_encoded_values() {
        let parsed = parse_oauth_callback_query(
            "code=abc123&state=hello%20world&error_description=needs%20consent",
        )
        .expect("query should parse");
        assert_eq!(
            parsed,
            OAuthCallbackParams {
                code: Some("abc123".into()),
                state: Some("hello world".into()),
                error: None,
                error_description: Some("needs consent".into()),
            }
        );
    }

    #[test]
    fn parse_callback_target_decodes_plus_delimited_spaces() {
        let parsed = parse_oauth_callback_request_target(
            "/callback?code=abc123&state=hello+world&error_description=needs%2Fconsent",
        )
        .expect("request target should parse");
        assert_eq!(parsed.state.as_deref(), Some("hello world"));
        assert_eq!(parsed.error_description.as_deref(), Some("needs/consent"));
    }

    #[test]
    fn parse_callback_target_accepts_oauth_callback_path() {
        // Antigravity's fixed loopback redirect ends in `/oauth-callback`; the
        // allowlist must accept it alongside the Gemini CLI `/oauth2callback`
        // and the generic `/callback`, or the browser leg of `/login google`
        // would 400 with "unexpected callback path".
        let parsed = parse_oauth_callback_request_target("/oauth-callback?code=xyz&state=s")
            .expect("oauth-callback target should parse");
        assert_eq!(parsed.code.as_deref(), Some("xyz"));
        assert_eq!(parsed.state.as_deref(), Some("s"));
    }
}
