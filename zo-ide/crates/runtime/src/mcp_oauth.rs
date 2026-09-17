use std::future::Future;
use std::io;
use std::net::TcpListener;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{McpOAuthConfig, McpRemoteServerConfig};
use crate::oauth::{
    credentials_path, generate_pkce_pair, generate_state, is_mcp_token_expired,
    load_mcp_oauth_token, loopback_redirect_uri, parse_oauth_callback_request_target,
    save_mcp_oauth_token, OAuthTokenSet, PkceCodePair,
};
use core_types::{OAuthAuthorizationRequest, OAuthConfig, OAuthTokenExchangeRequest};

const DEFAULT_CALLBACK_PORT: u16 = 18_923;
const DEFAULT_AUTHORIZE_URL: &str = "https://auth.example.com/oauth/authorize";
const DEFAULT_TOKEN_URL: &str = "https://auth.example.com/oauth/token";

/// Trait for opening a browser URL. CLI provides the real implementation.
pub trait BrowserOpener: Send + Sync {
    fn open_url(&self, url: &str) -> io::Result<()>;
}

/// Result of an MCP OAuth authentication attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpAuthResult {
    /// Already authenticated with a valid token.
    AlreadyAuthenticated { server: String },
    /// Successfully completed OAuth flow.
    Authenticated { server: String, scopes: Vec<String> },
    /// Token was refreshed without user interaction.
    Refreshed { server: String },
    /// OAuth flow failed.
    Failed { server: String, reason: String },
}

/// Build an [`OAuthConfig`] from the MCP-specific config, filling defaults.
///
/// When `auth_server_metadata_url` is set, this function attempts RFC 8414
/// metadata discovery: it fetches the URL as JSON and extracts
/// `authorization_endpoint` and `token_endpoint`.  If the fetch fails, the
/// metadata URL is used directly as the authorize URL for backward
/// compatibility.
#[must_use]
pub fn mcp_oauth_to_config(server_name: &str, mcp_oauth: &McpOAuthConfig) -> OAuthConfig {
    let metadata = mcp_oauth
        .auth_server_metadata_url
        .as_ref()
        .and_then(|url| fetch_oauth_metadata(url).ok());

    let (authorize_url, token_url) = match (&metadata, &mcp_oauth.auth_server_metadata_url) {
        (Some(meta), _) => (
            meta.authorization_endpoint.clone(),
            meta.token_endpoint.clone(),
        ),
        // Metadata URL set but discovery failed: use it directly as the authorize
        // URL (backward compat for servers that don't serve RFC 8414).
        (None, Some(metadata_url)) => (metadata_url.clone(), DEFAULT_TOKEN_URL.to_string()),
        (None, None) => (
            DEFAULT_AUTHORIZE_URL.to_string(),
            DEFAULT_TOKEN_URL.to_string(),
        ),
    };

    let port = mcp_oauth.callback_port.unwrap_or(DEFAULT_CALLBACK_PORT);
    let (client_id, client_secret) =
        resolve_client_credentials(server_name, mcp_oauth, metadata.as_ref(), port);

    OAuthConfig {
        client_id,
        authorize_url,
        token_url,
        callback_port: mcp_oauth.callback_port,
        manual_redirect_url: None,
        scopes: Vec::new(),
        client_secret,
    }
}

/// RFC 8414 OAuth Authorization Server Metadata (the subset zo uses).
#[derive(Debug, Clone)]
#[allow(clippy::struct_field_names)]
struct OAuthServerMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    /// RFC 7591 Dynamic Client Registration endpoint, when advertised.
    registration_endpoint: Option<String>,
}

/// A client registered via RFC 7591 DCR, cached in `registered_clients.json` so
/// zo registers once per MCP server rather than on every auth attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegisteredClient {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

/// Fetch RFC 8414 OAuth Authorization Server Metadata and extract the endpoints
/// zo needs (authorization, token, and — for DCR — registration).
fn fetch_oauth_metadata(metadata_url: &str) -> io::Result<OAuthServerMetadata> {
    let body: serde_json::Value = run_http(async {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(io::Error::other)?;

        let response = client
            .get(metadata_url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;

        if !response.status().is_success() {
            return Err(io::Error::other(format!(
                "OAuth metadata fetch from {metadata_url} failed: HTTP {}",
                response.status()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    })?;

    let authorization_endpoint = body["authorization_endpoint"]
        .as_str()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "OAuth metadata missing authorization_endpoint",
            )
        })?
        .to_string();

    let token_endpoint = body["token_endpoint"]
        .as_str()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "OAuth metadata missing token_endpoint",
            )
        })?
        .to_string();

    let registration_endpoint = body["registration_endpoint"].as_str().map(str::to_string);

    Ok(OAuthServerMetadata {
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
    })
}

/// Validate endpoints learned from an untrusted discovery document before
/// opening a browser or sending credentials. Explicit user OAuth settings keep
/// their existing behavior; this boundary applies only to remote discovery.
fn validate_discovered_oauth_metadata(
    metadata: &OAuthServerMetadata,
) -> io::Result<OAuthServerMetadata> {
    Ok(OAuthServerMetadata {
        authorization_endpoint: require_web_url(
            &metadata.authorization_endpoint,
            "authorization_endpoint",
        )?,
        token_endpoint: require_web_url(&metadata.token_endpoint, "token_endpoint")?,
        registration_endpoint: metadata
            .registration_endpoint
            .as_deref()
            .map(|url| require_web_url(url, "registration_endpoint"))
            .transpose()?,
    })
}

/// Resolve the OAuth client for an MCP server: an explicitly configured
/// `client_id` wins (the user manages its secret); otherwise reuse a cached DCR
/// registration; otherwise, if the server advertises a `registration_endpoint`,
/// register dynamically (RFC 7591) and cache it; otherwise fall back to a public
/// default client id.
fn resolve_client_credentials(
    server_name: &str,
    mcp_oauth: &McpOAuthConfig,
    metadata: Option<&OAuthServerMetadata>,
    port: u16,
) -> (String, Option<String>) {
    if let Some(client_id) = &mcp_oauth.client_id {
        return (client_id.clone(), None);
    }
    if let Ok(cached) = load_registered_client(server_name) {
        return (cached.client_id, cached.client_secret);
    }
    if let Some(registration_endpoint) = metadata.and_then(|m| m.registration_endpoint.as_deref()) {
        if let Ok(registered) = run_http(register_mcp_client_via_dcr(registration_endpoint, port)) {
            // Cache failures are non-fatal — we just re-register next time.
            let _ = save_registered_client(server_name, &registered);
            return (registered.client_id, registered.client_secret);
        }
    }
    ("mcp-client".to_string(), None)
}

/// Register a confidential client via RFC 7591 Dynamic Client Registration,
/// requesting `client_secret_post` so the token exchange can authenticate.
async fn register_mcp_client_via_dcr(
    registration_endpoint: &str,
    port: u16,
) -> io::Result<RegisteredClient> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(io::Error::other)?;

    let redirect_uri = loopback_redirect_uri(port);
    let registration = serde_json::json!({
        "redirect_uris": [redirect_uri],
        "token_endpoint_auth_method": "client_secret_post",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "client_name": "zo",
        "application_type": "native",
    });

    let response = client
        .post(registration_endpoint)
        .json(&registration)
        .send()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;

    if !response.status().is_success() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "dynamic client registration failed: HTTP {}",
                response.status()
            ),
        ));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let client_id = body["client_id"]
        .as_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "DCR response missing client_id"))?
        .to_string();
    let client_secret = body["client_secret"].as_str().map(str::to_string);

    Ok(RegisteredClient {
        client_id,
        client_secret,
    })
}

/// Path to the DCR cache, a 0600 sibling of `credentials.json`.
fn registered_clients_path() -> io::Result<std::path::PathBuf> {
    Ok(credentials_path()?.with_file_name("registered_clients.json"))
}

fn load_registered_client(server_name: &str) -> io::Result<RegisteredClient> {
    let root = api::oauth_store::read_credentials_root(&registered_clients_path()?)?;
    let entry = root
        .get(server_name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no cached registered client"))?;
    serde_json::from_value(entry.clone())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn save_registered_client(server_name: &str, client: &RegisteredClient) -> io::Result<()> {
    api::oauth_store::update_credentials_root(&registered_clients_path()?, |root| {
        let value = serde_json::to_value(client)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        root.insert(server_name.to_owned(), value);
        Ok(())
    })
}

/// Attempt to authenticate with an MCP server.
///
/// 1. Check for a cached, non-expired token.
/// 2. If expired but `refresh_token` exists, attempt refresh.
/// 3. Otherwise, run the full browser-based PKCE flow.
pub fn authenticate_mcp_server(
    server_name: &str,
    mcp_oauth: &McpOAuthConfig,
    browser: &dyn BrowserOpener,
) -> McpAuthResult {
    // Fast path: a cached, valid token needs no network (and no config build).
    let cached = load_mcp_oauth_token(server_name).ok().flatten();
    if let Some(token) = &cached {
        if !is_mcp_token_expired(token) {
            return McpAuthResult::AlreadyAuthenticated {
                server: server_name.to_string(),
            };
        }
    }

    // Build the OAuth config ONCE here, in this top-level synchronous context.
    // Metadata discovery + DCR (each a `run_http`) happen only here, so the async
    // refresh path below never nests a `run_http` inside `run_http` — which would
    // panic with "Cannot start a runtime from within a runtime".
    let config = mcp_oauth_to_config(server_name, mcp_oauth);

    complete_mcp_auth(server_name, &config, cached.as_ref(), browser)
}

/// Authenticate a remote (HTTP/SSE) MCP server.
///
/// An explicit per-server `oauth` block is highest priority and is handled
/// exactly like [`authenticate_mcp_server`]. When it is absent, this performs
/// bounded native OAuth discovery against the server's endpoint (probe for a
/// `401` Bearer challenge, resolve RFC 9728 / RFC 8414 metadata) and drives the
/// same refresh/PKCE flow with the discovered endpoints. The obtained token is
/// stored under `server_name`, so subsequent MCP requests inject it regardless
/// of whether settings ever declared an `oauth` block.
pub fn authenticate_mcp_server_remote(
    server_name: &str,
    remote: &McpRemoteServerConfig,
    browser: &dyn BrowserOpener,
) -> McpAuthResult {
    // Fast path first: a cached, valid token short-circuits before any network
    // discovery is attempted.
    let cached = load_mcp_oauth_token(server_name).ok().flatten();
    if let Some(token) = &cached {
        if !is_mcp_token_expired(token) {
            return McpAuthResult::AlreadyAuthenticated {
                server: server_name.to_string(),
            };
        }
    }

    // Explicit config wins and is unchanged; otherwise discover natively. Both
    // build the config in this top-level synchronous context so the refresh path
    // never nests a `run_http` inside `run_http`.
    let config = match &remote.oauth {
        Some(mcp_oauth) => mcp_oauth_to_config(server_name, mcp_oauth),
        None => match discover_oauth_config(server_name, &remote.url) {
            Ok(config) => config,
            Err(error) => {
                return McpAuthResult::Failed {
                    server: server_name.to_string(),
                    reason: error.to_string(),
                };
            }
        },
    };

    complete_mcp_auth(server_name, &config, cached.as_ref(), browser)
}

/// Shared refresh-then-PKCE tail for both the explicit-config and native
/// discovery entry points. A present `refresh_token` on `cached` is tried before
/// falling back to the interactive browser flow.
fn complete_mcp_auth(
    server_name: &str,
    config: &OAuthConfig,
    cached: Option<&OAuthTokenSet>,
    browser: &dyn BrowserOpener,
) -> McpAuthResult {
    // Try refresh when we have a refresh token.
    if let Some(token) = cached {
        if let Some(refresh_token) = &token.refresh_token {
            if let Ok(result) = run_http(try_refresh_token(server_name, config, refresh_token)) {
                return result;
            }
            // fall through to full flow on refresh failure
        }
    }

    // Full PKCE authorization code flow
    match run_pkce_flow(server_name, config, browser) {
        Ok(result) => result,
        Err(error) => McpAuthResult::Failed {
            server: server_name.to_string(),
            reason: error.to_string(),
        },
    }
}

/// Bounded timeout for every native-discovery HTTP probe/fetch, in seconds.
const DISCOVERY_TIMEOUT_SECS: u64 = 10;

/// A parsed `WWW-Authenticate: Bearer …` challenge — the subset of RFC 6750 /
/// RFC 9728 parameters native OAuth discovery needs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct BearerChallenge {
    /// RFC 9728 protected-resource metadata URL, when advertised.
    resource_metadata: Option<String>,
    /// A direct authorization-server (RFC 8414) metadata URL hint.
    authorization_uri: Option<String>,
    /// The `error` code (e.g. `invalid_token`), retained for diagnostics only.
    error: Option<String>,
}

/// Perform bounded protocol-level OAuth discovery for a configured remote MCP
/// server that declares no explicit `oauth` block, returning a fully-resolved
/// [`OAuthConfig`].
///
/// The endpoint is probed for a `401` Bearer challenge; its parameters resolve an
/// RFC 8414 authorization-server metadata URL (via RFC 9728 protected-resource
/// metadata when advertised, else the endpoint origin's well-known document).
/// That metadata is fetched and validated, and a client id is resolved (cached
/// DCR, dynamic registration, or the public default) — so a discovered server
/// never falls back to placeholder `example.com` endpoints.
fn discover_oauth_config(server_name: &str, endpoint_url: &str) -> io::Result<OAuthConfig> {
    let endpoint = require_web_url(endpoint_url, "MCP endpoint")?;
    let challenge = run_http(probe_oauth_challenge(&endpoint))?;
    let metadata_url = resolve_authorization_metadata_url(&endpoint, &challenge)?;
    let metadata = fetch_oauth_metadata(&metadata_url)?;
    let metadata = validate_discovered_oauth_metadata(&metadata)?;

    let discovered = McpOAuthConfig {
        client_id: None,
        callback_port: None,
        auth_server_metadata_url: Some(metadata_url),
        xaa: None,
    };
    let (client_id, client_secret) = resolve_client_credentials(
        server_name,
        &discovered,
        Some(&metadata),
        DEFAULT_CALLBACK_PORT,
    );

    Ok(OAuthConfig {
        client_id,
        authorize_url: metadata.authorization_endpoint,
        token_url: metadata.token_endpoint,
        callback_port: None,
        manual_redirect_url: None,
        scopes: Vec::new(),
        client_secret,
    })
}

/// Probe a remote MCP endpoint for an OAuth (`401` Bearer) challenge.
///
/// The primary probe is a minimal valid JSON-RPC `initialize` POST carrying a
/// JSON content type and MCP-appropriate `Accept`. If that yields no Bearer
/// challenge, an SSE `text/event-stream` GET is tried as a fallback for servers
/// that only challenge on the event stream.
async fn probe_oauth_challenge(endpoint_url: &str) -> io::Result<BearerChallenge> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(DISCOVERY_TIMEOUT_SECS))
        .build()
        .map_err(io::Error::other)?;

    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "zo", "version": env!("CARGO_PKG_VERSION") },
        },
    });

    let post = client
        .post(endpoint_url)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .json(&initialize)
        .send()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
    if let Some(challenge) = bearer_challenge_from_response(&post) {
        return Ok(challenge);
    }

    let get = client
        .get(endpoint_url)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
    if let Some(challenge) = bearer_challenge_from_response(&get) {
        return Ok(challenge);
    }

    Err(io::Error::other(format!(
        "MCP server at {endpoint_url} did not issue an OAuth challenge \
         (no `WWW-Authenticate: Bearer` on initialize); it may not require OAuth \
         or expects a pre-provisioned token"
    )))
}

/// Extract a Bearer challenge from a response's `WWW-Authenticate` header, if any.
fn bearer_challenge_from_response(response: &reqwest::Response) -> Option<BearerChallenge> {
    let value = response
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)?
        .to_str()
        .ok()?;
    parse_bearer_challenge(value)
}

/// Parse a `WWW-Authenticate` header value into a [`BearerChallenge`].
///
/// The scheme is matched case-insensitively; a non-Bearer challenge yields
/// `None`. Parameter names are matched case-insensitively and values may be
/// quoted (with escapes) or bare tokens.
fn parse_bearer_challenge(header_value: &str) -> Option<BearerChallenge> {
    let rest = strip_bearer_scheme(header_value)?;
    let params = parse_www_auth_params(rest);
    let lookup = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    Some(BearerChallenge {
        resource_metadata: lookup("resource_metadata"),
        authorization_uri: lookup("authorization_uri"),
        error: lookup("error"),
    })
}

/// Return the parameter list following a leading `Bearer` scheme token
/// (case-insensitive), or `None` when the challenge is not a Bearer challenge.
fn strip_bearer_scheme(header_value: &str) -> Option<&str> {
    let trimmed = header_value.trim_start();
    let scheme_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let (scheme, rest) = trimmed.split_at(scheme_end);
    scheme.eq_ignore_ascii_case("bearer").then_some(rest)
}

/// Parse `key=value` / `key="quoted value"` auth parameters (RFC 7235 style)
/// into ordered pairs. Quoted values may contain commas and backslash escapes.
fn parse_www_auth_params(input: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars
            .peek()
            .is_some_and(|c| c.is_whitespace() || *c == ',')
        {
            chars.next();
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c == ',' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }
        if key.is_empty() {
            break;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek() != Some(&'=') {
            // A bare token (e.g. `token68`) with no value — ignore and continue.
            continue;
        }
        chars.next();
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(escaped) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    '"' => break,
                    _ => value.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ',' || c.is_whitespace() {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        params.push((key, value));
    }
    params
}

/// Resolve an RFC 8414 authorization-server metadata URL from a Bearer challenge.
///
/// When the challenge advertises `resource_metadata`, RFC 9728 protected-resource
/// metadata is fetched and its first authorization server resolved — unless a
/// usable `authorization_uri` hint is present, which is honored directly. Without
/// `resource_metadata`, the endpoint origin's `/.well-known/oauth-authorization-server`
/// is used as the protocol-level origin fallback.
fn resolve_authorization_metadata_url(
    endpoint_url: &str,
    challenge: &BearerChallenge,
) -> io::Result<String> {
    if let Some(resource_metadata) = &challenge.resource_metadata {
        let resource_metadata = require_web_url(resource_metadata, "resource_metadata")?;
        if let Some(hint) = &challenge.authorization_uri {
            if let Ok(hint) = require_web_url(hint, "authorization_uri") {
                return Ok(hint);
            }
        }
        let protected = run_http(fetch_json(&resource_metadata))?;
        let issuer = protected
            .get("authorization_servers")
            .and_then(serde_json::Value::as_array)
            .and_then(|servers| servers.first())
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "protected-resource metadata at {resource_metadata} lists no authorization_servers"
                    ),
                )
            })?;
        let issuer = require_web_url(issuer, "authorization server issuer")?;
        return Ok(well_known_oauth_metadata_url(&issuer));
    }

    let origin = origin_of(endpoint_url)?;
    Ok(format!("{origin}/.well-known/oauth-authorization-server"))
}

/// Build an RFC 8414 metadata URL for an authorization-server `issuer`. An issuer
/// that already points at a well-known document is used as-is; otherwise the
/// standard well-known suffix is appended.
fn well_known_oauth_metadata_url(issuer: &str) -> String {
    let issuer = issuer.trim_end_matches('/');
    if issuer.contains("/.well-known/") {
        return issuer.to_string();
    }
    format!("{issuer}/.well-known/oauth-authorization-server")
}

/// Fetch and JSON-decode a discovery document with a bounded timeout.
async fn fetch_json(url: &str) -> io::Result<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(DISCOVERY_TIMEOUT_SECS))
        .build()
        .map_err(io::Error::other)?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
    if !response.status().is_success() {
        return Err(io::Error::other(format!(
            "metadata fetch from {url} failed: HTTP {}",
            response.status()
        )));
    }
    response
        .json()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Accept only `http(s)` discovery URLs, rejecting any other scheme so a hostile
/// challenge cannot steer discovery at `file:`, `data:`, etc. The URL is echoed
/// (URLs are not secret) but tokens never are.
fn require_web_url(url: &str, what: &str) -> io::Result<String> {
    let trimmed = url.trim();
    let parsed = reqwest::Url::parse(trimmed).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid MCP OAuth {what}: {error}"),
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("MCP OAuth {what} must be an HTTP(S) URL without credentials"),
        ));
    }
    Ok(parsed.into())
}

/// Extract the `scheme://authority` origin of an `http(s)` URL.
fn origin_of(url: &str) -> io::Result<String> {
    let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("MCP endpoint URL is not HTTP(S): {url}"),
        ));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
    if authority.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("MCP endpoint URL has no host: {url}"),
        ));
    }
    Ok(format!("{scheme}://{authority}"))
}

/// Attempt to refresh an MCP server token.
async fn try_refresh_token(
    server_name: &str,
    config: &OAuthConfig,
    refresh_token: &str,
) -> io::Result<McpAuthResult> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(io::Error::other)?;

    let refresh_req = core_types::OAuthRefreshRequest::from_config(config, refresh_token, None);
    let response = client
        .post(&config.token_url)
        .form(&refresh_req.form_params())
        .send()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;

    if !response.status().is_success() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("token refresh failed: HTTP {}", response.status()),
        ));
    }

    let token_set = parse_token_response(response).await?;
    save_mcp_oauth_token(server_name, &token_set)?;

    Ok(McpAuthResult::Refreshed {
        server: server_name.to_string(),
    })
}

/// Run the full PKCE authorization code flow.
fn run_pkce_flow(
    server_name: &str,
    config: &OAuthConfig,
    browser: &dyn BrowserOpener,
) -> io::Result<McpAuthResult> {
    let port = config.callback_port.unwrap_or(DEFAULT_CALLBACK_PORT);
    let pkce = generate_pkce_pair()?;
    let state = generate_state()?;
    let redirect_uri = loopback_redirect_uri(port);

    let auth_url =
        OAuthAuthorizationRequest::from_config(config, redirect_uri.clone(), &state, &pkce)
            .build_url();

    browser.open_url(&auth_url)?;

    let callback = wait_for_callback(port)?;

    if callback.state.as_deref() != Some(state.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "OAuth state mismatch",
        ));
    }

    if let Some(ref error) = callback.error {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "OAuth error: {error}{}",
                callback
                    .error_description
                    .as_deref()
                    .map_or(String::new(), |d| format!(" — {d}"))
            ),
        ));
    }

    let code = callback
        .code
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "OAuth callback missing code"))?;

    let token_set = run_http(exchange_code(
        server_name,
        config,
        &code,
        &state,
        pkce,
        &redirect_uri,
    ))?;
    let scopes = token_set.scopes.clone();
    save_mcp_oauth_token(server_name, &token_set)?;

    Ok(McpAuthResult::Authenticated {
        server: server_name.to_string(),
        scopes,
    })
}

/// Exchange an authorization code for tokens.
async fn exchange_code(
    _server_name: &str,
    config: &OAuthConfig,
    code: &str,
    state: &str,
    pkce: PkceCodePair,
    redirect_uri: &str,
) -> io::Result<OAuthTokenSet> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(io::Error::other)?;

    let exchange = OAuthTokenExchangeRequest::from_config(
        config,
        code,
        state,
        pkce.verifier,
        redirect_uri.to_string(),
    );

    let response = client
        .post(&config.token_url)
        .form(&exchange.form_params())
        .send()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;

    if !response.status().is_success() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("token exchange failed: HTTP {}", response.status()),
        ));
    }

    parse_token_response(response).await
}

/// Parse a token endpoint JSON response into an `OAuthTokenSet`.
async fn parse_token_response(response: reqwest::Response) -> io::Result<OAuthTokenSet> {
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing access_token"))?
        .to_string();

    let refresh_token = body["refresh_token"].as_str().map(ToString::to_string);
    let expires_in = body["expires_in"].as_u64();
    let expires_at = expires_in.map(|seconds| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() + seconds)
            .unwrap_or(0)
    });

    let scopes: Vec<String> = body["scope"]
        .as_str()
        .map(|s| s.split_whitespace().map(ToString::to_string).collect())
        .unwrap_or_default();

    Ok(OAuthTokenSet {
        access_token,
        refresh_token,
        expires_at,
        scopes,
    })
}

/// Timeout for the OAuth callback listener (120 seconds).
const OAUTH_CALLBACK_TIMEOUT_SECS: u64 = 120;

/// Wait for an OAuth callback on the loopback address.
fn wait_for_callback(port: u16) -> io::Result<core_types::OAuthCallbackParams> {
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))?;
    // Use non-blocking + poll loop to enforce a deadline on accept().
    listener.set_nonblocking(true)?;

    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(OAUTH_CALLBACK_TIMEOUT_SECS);

    let (mut stream, _addr) = loop {
        match listener.accept() {
            Ok(conn) => break conn,
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "OAuth callback timed out after {OAUTH_CALLBACK_TIMEOUT_SECS}s \
                             — no browser redirect received on port {port}"
                        ),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    };

    let mut buf = vec![0u8; 4096];
    let n = io::Read::read(&mut stream, &mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let first_line = request.lines().next().unwrap_or("");
    let target = first_line.split_whitespace().nth(1).unwrap_or("/callback");

    // Send a minimal response before parsing
    let response_body = "Authentication complete. You may close this tab.";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
        response_body.len()
    );
    io::Write::write_all(&mut stream, response.as_bytes())?;

    parse_oauth_callback_request_target(target)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Drive an async HTTP future from the synchronous MCP OAuth entry points.
///
/// Delegates to the canonical [`api::sync_bridge::run_blocking`]. The old
/// inline copy here called a bare `Handle::block_on` with no runtime-flavor
/// check and built a fresh runtime per call when no ambient one existed —
/// the per-copy drift the shared bridge exists to eliminate.
fn run_http<F, T>(future: F) -> io::Result<T>
where
    F: Future<Output = io::Result<T>>,
{
    api::sync_bridge::run_blocking(future)
}

/// Get a valid Bearer token for an MCP server, loading from cache.
///
/// Returns `None` if no token is stored or the token is expired.
#[must_use]
pub fn get_mcp_bearer_token(server_name: &str) -> Option<String> {
    let token = load_mcp_oauth_token(server_name).ok()??;
    if is_mcp_token_expired(&token) {
        return None;
    }
    Some(token.access_token)
}

/// Browser opener that shells out to the platform default browser command.
///
/// Shared by the `McpAuth` tool and the `/mcp auth <server>` command so the
/// interactive OAuth flow opens the user's browser the same way everywhere.
pub struct LocalBrowserOpener;

impl BrowserOpener for LocalBrowserOpener {
    fn open_url(&self, url: &str) -> io::Result<()> {
        open_browser(url)
    }
}

/// Spawn the platform's default browser opener (`open` on macOS, `start` via
/// `cmd` on Windows, `xdg-open` elsewhere) on `url`, returning the first
/// success or an error if no opener is available.
///
/// Single source of truth for launching the user's browser during interactive
/// OAuth flows — shared by the MCP OAuth [`LocalBrowserOpener`] and the
/// provider-login flow in the CLI.
///
/// # Errors
/// Returns an `io::Error` if a launcher is found but fails, or
/// `NotFound` if no supported opener command exists.
pub fn open_browser(url: &str) -> io::Result<()> {
    let commands = if cfg!(target_os = "macos") {
        vec![("open", vec![url])]
    } else if cfg!(target_os = "windows") {
        vec![("cmd", vec!["/C", "start", "", url])]
    } else {
        vec![("xdg-open", vec![url])]
    };

    for (program, args) in commands {
        match Command::new(program).args(args).spawn() {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no supported browser opener command found",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_oauth_to_config_fills_defaults() {
        let mcp = McpOAuthConfig {
            client_id: None,
            callback_port: Some(9999),
            auth_server_metadata_url: None,
            xaa: None,
        };
        let config = mcp_oauth_to_config("mcp-oauth-defaults-test", &mcp);
        assert_eq!(config.client_id, "mcp-client");
        assert_eq!(config.callback_port, Some(9999));
    }

    #[test]
    fn mcp_oauth_to_config_uses_provided_values() {
        let mcp = McpOAuthConfig {
            client_id: Some("my-client".to_string()),
            callback_port: None,
            auth_server_metadata_url: Some("https://auth.test/authorize".to_string()),
            xaa: Some(true),
        };
        let config = mcp_oauth_to_config("mcp-oauth-provided-test", &mcp);
        assert_eq!(config.client_id, "my-client");
        assert_eq!(config.authorize_url, "https://auth.test/authorize");
    }

    #[test]
    fn get_mcp_bearer_token_returns_none_when_no_token() {
        // Use a server name that won't have stored tokens
        assert!(get_mcp_bearer_token("nonexistent-test-server-12345").is_none());
    }

    struct FakeBrowser {
        opened_url: std::sync::Mutex<Option<String>>,
    }

    impl BrowserOpener for FakeBrowser {
        fn open_url(&self, url: &str) -> io::Result<()> {
            *self.opened_url.lock().unwrap() = Some(url.to_string());
            Ok(())
        }
    }

    #[test]
    fn authenticate_returns_already_authenticated_for_valid_cached_token() {
        let _guard = crate::test_env_lock();
        let config_home = std::env::temp_dir().join(format!(
            "mcp-oauth-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &config_home);

        let token = OAuthTokenSet {
            access_token: "valid-token".to_string(),
            refresh_token: None,
            expires_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 3600,
            ),
            scopes: vec!["read".to_string()],
        };
        save_mcp_oauth_token("test-server", &token).expect("save");

        let browser = FakeBrowser {
            opened_url: std::sync::Mutex::new(None),
        };
        let mcp_config = McpOAuthConfig {
            client_id: Some("test".to_string()),
            callback_port: None,
            auth_server_metadata_url: None,
            xaa: None,
        };

        let result = authenticate_mcp_server("test-server", &mcp_config, &browser);
        assert!(matches!(result, McpAuthResult::AlreadyAuthenticated { .. }));

        // Browser should NOT have been opened
        assert!(browser.opened_url.lock().unwrap().is_none());

        std::env::remove_var("ZO_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).ok();
    }

    // --- Remote MCP OAuth protocol discovery ------------------------------

    #[test]
    fn parse_bearer_challenge_is_case_insensitive_with_quoted_params() {
        let challenge = parse_bearer_challenge(
            r#"bearer REALM="OAuth", Error="invalid_token", Resource_Metadata="https://mcp.example.test/.well-known/oauth-protected-resource""#,
        )
        .expect("a Bearer challenge");
        assert_eq!(
            challenge.resource_metadata.as_deref(),
            Some("https://mcp.example.test/.well-known/oauth-protected-resource")
        );
        assert_eq!(challenge.error.as_deref(), Some("invalid_token"));
    }

    #[test]
    fn parse_bearer_challenge_reads_authorization_uri_hint() {
        let challenge = parse_bearer_challenge(
            r#"Bearer error="invalid_token", resource_metadata="https://mcp.example.test/.well-known/oauth-protected-resource", authorization_uri="https://auth.example.test/.well-known/oauth-authorization-server""#,
        )
        .expect("a Bearer challenge");
        assert_eq!(
            challenge.authorization_uri.as_deref(),
            Some("https://auth.example.test/.well-known/oauth-authorization-server")
        );
    }

    #[test]
    fn parse_bearer_challenge_rejects_non_bearer_scheme() {
        assert!(parse_bearer_challenge(r#"Basic realm="corp""#).is_none());
    }

    #[test]
    fn resolve_metadata_url_falls_back_to_origin_well_known() {
        // Some servers issue a Bearer challenge without `resource_metadata`.
        let challenge = BearerChallenge {
            resource_metadata: None,
            authorization_uri: None,
            error: Some("invalid_token".to_string()),
        };
        let url = resolve_authorization_metadata_url("https://mcp.example.test/v1/sse", &challenge)
            .expect("origin fallback");
        assert_eq!(
            url,
            "https://mcp.example.test/.well-known/oauth-authorization-server"
        );
    }

    #[test]
    fn resolve_metadata_url_honors_authorization_uri_hint() {
        // A usable authorization_uri hint short-circuits the PRM fetch (no network).
        let challenge = BearerChallenge {
            resource_metadata: Some(
                "https://mcp.example.test/.well-known/oauth-protected-resource".to_string(),
            ),
            authorization_uri: Some(
                "https://auth.example.test/.well-known/oauth-authorization-server".to_string(),
            ),
            error: None,
        };
        let url = resolve_authorization_metadata_url("https://mcp.example.test/mcp", &challenge)
            .expect("hint honored");
        assert_eq!(
            url,
            "https://auth.example.test/.well-known/oauth-authorization-server"
        );
    }

    #[test]
    fn require_web_url_rejects_non_http_schemes() {
        assert!(require_web_url("file:///etc/passwd", "test").is_err());
        assert!(require_web_url("ftp://host/x", "test").is_err());
        assert!(require_web_url("https://user:secret@host/x", "test").is_err());
        assert!(require_web_url("https://", "test").is_err());
        assert_eq!(
            require_web_url("  https://ok.example/x  ", "test").expect("https accepted"),
            "https://ok.example/x"
        );
    }

    #[test]
    fn origin_of_extracts_scheme_host_and_port() {
        assert_eq!(
            origin_of("https://mcp.example.test/v1/sse?x=1").expect("origin"),
            "https://mcp.example.test"
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8080/rpc").expect("origin"),
            "http://127.0.0.1:8080"
        );
        assert!(origin_of("ftp://host/y").is_err());
    }

    #[test]
    fn discover_rejects_non_http_endpoint() {
        let error =
            discover_oauth_config("scheme-test", "ftp://example.com/mcp").expect_err("rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn discover_errors_when_no_challenge_is_issued() {
        let base = spawn_mock_server(|_base| {
            |_method: &str, _path: &str, _body: &str| -> MockResponse { MockResponse::ok_empty() }
        });
        let error = discover_oauth_config("no-oauth-test", &format!("{base}/mcp"))
            .expect_err("must fail without a challenge");
        assert!(
            error.to_string().contains("did not issue an OAuth challenge"),
            "actionable error expected, got: {error}"
        );
    }

    #[test]
    fn discover_origin_metadata_fallback_resolves_endpoints() {
        let _guard = crate::test_env_lock();
        let home = unique_temp("discover-origin-fallback");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let base = spawn_mock_server(|base| {
            let base = base.to_owned();
            move |method: &str, path: &str, _body: &str| -> MockResponse {
                if method == "POST" {
                    // The challenge intentionally omits RFC 9728 resource metadata.
                    return MockResponse::unauthorized(
                        r#"Bearer realm="OAuth", error="invalid_token""#,
                    );
                }
                if path == "/.well-known/oauth-authorization-server" {
                    return MockResponse::json(&serde_json::json!({
                        "issuer": base,
                        "authorization_endpoint": format!("{base}/authorize"),
                        "token_endpoint": format!("{base}/token"),
                    }));
                }
                MockResponse::ok_empty()
            }
        });

        let config = discover_oauth_config("origin-fallback-test", &format!("{base}/v1/sse"))
            .expect("discovery resolves via the origin well-known document");
        assert_eq!(config.authorize_url, format!("{base}/authorize"));
        assert_eq!(config.token_url, format!("{base}/token"));
        assert_eq!(config.client_id, "mcp-client");
        assert!(
            !config.token_url.contains("example.com"),
            "discovered servers must not use placeholder endpoints"
        );

        std::env::remove_var("ZO_CONFIG_HOME");
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn discover_post_resource_metadata_resolves_endpoints() {
        let _guard = crate::test_env_lock();
        let home = unique_temp("discover-resource-metadata");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let base = spawn_mock_server(|base| {
            let base = base.to_owned();
            move |method: &str, path: &str, _body: &str| -> MockResponse {
                match (method, path) {
                    // This server challenges only the JSON-RPC POST and advertises
                    // RFC 9728 protected-resource metadata.
                    ("POST", _) => MockResponse::unauthorized(&format!(
                        r#"Bearer error="invalid_token", resource_metadata="{base}/.well-known/oauth-protected-resource""#
                    )),
                    ("GET", "/.well-known/oauth-protected-resource") => {
                        MockResponse::json(&serde_json::json!({
                            "resource": format!("{base}/mcp"),
                            "authorization_servers": [base],
                        }))
                    }
                    ("GET", "/.well-known/oauth-authorization-server") => {
                        MockResponse::json(&serde_json::json!({
                            "issuer": base,
                            "authorization_endpoint": format!("{base}/authorize"),
                            "token_endpoint": format!("{base}/token"),
                        }))
                    }
                    _ => MockResponse::ok_empty(),
                }
            }
        });

        let config = discover_oauth_config("resource-metadata-test", &format!("{base}/mcp"))
            .expect("discovery resolves via protected-resource metadata");
        assert_eq!(config.authorize_url, format!("{base}/authorize"));
        assert_eq!(config.token_url, format!("{base}/token"));

        std::env::remove_var("ZO_CONFIG_HOME");
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn discover_uses_sse_get_fallback_when_post_is_not_challenged() {
        let _guard = crate::test_env_lock();
        let home = unique_temp("discover-sse");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let base = spawn_mock_server(|base| {
            let base = base.to_owned();
            move |method: &str, path: &str, _body: &str| -> MockResponse {
                if path == "/.well-known/oauth-authorization-server" {
                    return MockResponse::json(&serde_json::json!({
                        "authorization_endpoint": format!("{base}/authorize"),
                        "token_endpoint": format!("{base}/token"),
                    }));
                }
                // Only the SSE event-stream GET challenges; the POST does not.
                if method == "GET" {
                    return MockResponse::unauthorized(r#"Bearer realm="OAuth""#);
                }
                MockResponse::ok_empty()
            }
        });

        let config = discover_oauth_config("sse-test", &format!("{base}/sse"))
            .expect("discovery falls back to the SSE GET probe");
        assert_eq!(config.token_url, format!("{base}/token"));

        std::env::remove_var("ZO_CONFIG_HOME");
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn authenticate_remote_uses_cached_token_without_discovery() {
        let _guard = crate::test_env_lock();
        let home = unique_temp("remote-fastpath");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let token = OAuthTokenSet {
            access_token: "cached-token".to_string(),
            refresh_token: None,
            expires_at: Some(unix_now_secs() + 3600),
            scopes: Vec::new(),
        };
        save_mcp_oauth_token("remote-fastpath", &token).expect("save");

        let browser = FakeBrowser {
            opened_url: std::sync::Mutex::new(None),
        };
        // A deliberately unreachable URL: discovery would fail, so returning
        // AlreadyAuthenticated proves the fast path skips the network entirely.
        let remote = McpRemoteServerConfig {
            url: "https://unreachable.invalid/mcp".to_string(),
            headers: std::collections::BTreeMap::new(),
            headers_helper: None,
            oauth: None,
        };

        let result = authenticate_mcp_server_remote("remote-fastpath", &remote, &browser);
        assert!(matches!(result, McpAuthResult::AlreadyAuthenticated { .. }));
        assert!(browser.opened_url.lock().unwrap().is_none());
        // A token stored under the configured server name is retrievable for
        // injection into subsequent remote MCP requests.
        assert_eq!(
            get_mcp_bearer_token("remote-fastpath").as_deref(),
            Some("cached-token")
        );

        std::env::remove_var("ZO_CONFIG_HOME");
        std::fs::remove_dir_all(home).ok();
    }

    // --- Discovery test harness -------------------------------------------

    fn unix_now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn unique_temp(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    struct MockResponse {
        status: u16,
        www_authenticate: Option<String>,
        body: String,
    }

    impl MockResponse {
        fn json(body: &serde_json::Value) -> Self {
            Self {
                status: 200,
                www_authenticate: None,
                body: body.to_string(),
            }
        }

        fn unauthorized(challenge: &str) -> Self {
            Self {
                status: 401,
                www_authenticate: Some(challenge.to_string()),
                body: "{}".to_string(),
            }
        }

        fn ok_empty() -> Self {
            Self {
                status: 200,
                www_authenticate: None,
                body: "{}".to_string(),
            }
        }

        fn to_http(&self) -> String {
            let reason = if self.status == 401 {
                "Unauthorized"
            } else {
                "OK"
            };
            let mut out = format!("HTTP/1.1 {} {reason}\r\n", self.status);
            if let Some(challenge) = &self.www_authenticate {
                out.push_str("WWW-Authenticate: ");
                out.push_str(challenge);
                out.push_str("\r\n");
            }
            out.push_str("Content-Type: application/json\r\n");
            out.push_str("Content-Length: ");
            out.push_str(&self.body.len().to_string());
            out.push_str("\r\nConnection: close\r\n\r\n");
            out.push_str(&self.body);
            out
        }
    }

    /// Spawn a single-connection-per-request mock HTTP server on a loopback port,
    /// routing each request through `make_handler` (which receives the server's
    /// own base URL so responses can embed absolute discovery URLs). Returns the
    /// base URL. The accept loop runs on a detached thread for the test's life.
    fn spawn_mock_server<F>(make_handler: impl FnOnce(&str) -> F) -> String
    where
        F: Fn(&str, &str, &str) -> MockResponse + Send + 'static,
    {
        use std::io::Write;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("mock server addr");
        let base = format!("http://{addr}");
        let handler = make_handler(&base);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                if let Some((method, path, body)) = read_http_request(&mut stream) {
                    let response = handler(&method, &path, &body);
                    let _ = stream.write_all(response.to_http().as_bytes());
                    let _ = stream.flush();
                }
            }
        });

        base
    }

    /// Read one HTTP request (method, path, body) from `stream`, honoring
    /// `Content-Length` for the body.
    fn read_http_request(stream: &mut std::net::TcpStream) -> Option<(String, String, String)> {
        use std::io::Read;

        let mut buf = Vec::new();
        let mut tmp = [0u8; 2048];
        let header_end = loop {
            let n = stream.read(&mut tmp).ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos;
            }
            if buf.len() > 64 * 1024 {
                return None;
            }
        };

        let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let mut request_line = head.lines().next().unwrap_or("").split_whitespace();
        let method = request_line.next().unwrap_or("").to_string();
        let path = request_line.next().unwrap_or("").to_string();

        let content_length = head
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);

        let body_start = header_end + 4;
        while buf.len() < body_start + content_length {
            let n = stream.read(&mut tmp).ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let end = (body_start + content_length).min(buf.len());
        let body = String::from_utf8_lossy(&buf[body_start..end]).to_string();

        Some((method, path, body))
    }
}
