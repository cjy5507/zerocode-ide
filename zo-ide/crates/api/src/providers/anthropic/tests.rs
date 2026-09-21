use crate::types::{BlockBinding, InputContentBlock, ToolChoice};
use super::{bind_thinking_for_wire, normalize_tool_choice_for_wire, with_beta, THINKING_BINDING_BETA};
use super::{ALT_REQUEST_ID_HEADER, REQUEST_ID_HEADER};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::oauth_store::{clear_oauth_credentials, save_oauth_credentials};
use core_types::OAuthConfig;

use super::{
    AnthropicClient, AuthSource, OAuthTokenSet, now_unix_timestamp, oauth_token_is_expired,
    resolve_saved_oauth_token_set_with, resolve_startup_auth_source,
};
use crate::types::{ContentBlockDelta, InputMessage, MessageRequest, StreamEvent};
use crate::ApiError;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_env_lock()
}

fn temp_config_home() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "api-oauth-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

fn cleanup_temp_config_home(config_home: &std::path::Path) {
    match std::fs::remove_dir_all(config_home) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("cleanup temp dir: {error}"),
    }
}

fn sample_oauth_config(token_url: String) -> OAuthConfig {
    OAuthConfig {
        client_id: "runtime-client".to_string(),
        authorize_url: "https://console.test/oauth/authorize".to_string(),
        token_url,
        callback_port: Some(4545),
        manual_redirect_url: Some("https://console.test/oauth/callback".to_string()),
        scopes: vec!["org:read".to_string(), "user:write".to_string()],
        client_secret: None,
    }
}

#[test]
fn read_api_key_requires_presence() {
    let _guard = env_lock();
    // Full credential-store isolation (HOME/ZO_HOME/ZO_CONFIG_HOME + env
    // fallbacks) so the developer machine's real saved OAuth token, which the
    // lower-root merge would otherwise pull in, cannot satisfy the lookup.
    let _isolation = crate::test_env::CredentialEnvIsolation::empty();
    let error = super::read_api_key().expect_err("missing key should error");
    assert!(matches!(
        error,
        crate::error::ApiError::MissingCredentials { .. }
    ));
}

#[test]
fn read_api_key_requires_non_empty_value() {
    let _guard = env_lock();
    let _isolation = crate::test_env::CredentialEnvIsolation::empty();
    std::env::set_var("ANTHROPIC_AUTH_TOKEN", "");
    let error = super::read_api_key().expect_err("empty key should error");
    assert!(matches!(
        error,
        crate::error::ApiError::MissingCredentials { .. }
    ));
}

#[test]
fn read_api_key_prefers_api_key_env() {
    let _guard = env_lock();
    std::env::set_var("ANTHROPIC_AUTH_TOKEN", "auth-token");
    std::env::set_var("ANTHROPIC_API_KEY", "legacy-key");
    assert_eq!(
        super::read_api_key().expect("api key should load"),
        "legacy-key"
    );
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    std::env::remove_var("ANTHROPIC_API_KEY");
}

#[test]
fn read_auth_token_reads_auth_token_env() {
    let _guard = env_lock();
    std::env::set_var("ANTHROPIC_AUTH_TOKEN", "auth-token");
    assert_eq!(super::read_auth_token().as_deref(), Some("auth-token"));
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
}

#[test]
fn oauth_token_maps_to_bearer_auth_source() {
    let auth = AuthSource::from(OAuthTokenSet {
        access_token: "access-token".to_string(),
        refresh_token: Some("refresh".to_string()),
        expires_at: Some(123),
        scopes: vec!["scope:a".to_string()],
    });
    assert_eq!(auth.bearer_token(), Some("access-token"));
    assert_eq!(auth.api_key(), None);
}

#[test]
fn auth_source_from_env_combines_api_key_and_bearer_token() {
    let _guard = env_lock();
    std::env::set_var("ANTHROPIC_AUTH_TOKEN", "auth-token");
    std::env::set_var("ANTHROPIC_API_KEY", "legacy-key");
    let auth = AuthSource::from_env().expect("env auth");
    assert_eq!(auth.api_key(), Some("legacy-key"));
    assert_eq!(auth.bearer_token(), Some("auth-token"));
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    std::env::remove_var("ANTHROPIC_API_KEY");
}

#[test]
fn auth_source_from_saved_oauth_when_env_absent() {
    let _guard = env_lock();
    let config_home = temp_config_home();
    std::env::set_var("ZO_CONFIG_HOME", &config_home);
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    std::env::remove_var("ANTHROPIC_API_KEY");
    save_oauth_credentials(&core_types::OAuthTokenSet {
        access_token: "saved-access-token".to_string(),
        refresh_token: Some("refresh".to_string()),
        expires_at: Some(now_unix_timestamp() + 300),
        scopes: vec!["scope:a".to_string()],
    })
    .expect("save oauth credentials");

    let auth = AuthSource::from_env_or_saved().expect("saved auth");
    assert_eq!(auth.bearer_token(), Some("saved-access-token"));

    clear_oauth_credentials().expect("clear credentials");
    std::env::remove_var("ZO_CONFIG_HOME");
    cleanup_temp_config_home(&config_home);
}

#[test]
fn oauth_token_expiry_uses_expires_at_timestamp() {
    // Already past → expired.
    assert!(oauth_token_is_expired(&OAuthTokenSet {
        access_token: "access-token".to_string(),
        refresh_token: None,
        expires_at: Some(1),
        scopes: Vec::new(),
    }));
    // Comfortably beyond the refresh buffer → still valid.
    assert!(!oauth_token_is_expired(&OAuthTokenSet {
        access_token: "access-token".to_string(),
        refresh_token: None,
        expires_at: Some(now_unix_timestamp() + 300),
        scopes: Vec::new(),
    }));
    // Inside the refresh buffer → treated as expired so we refresh early
    // rather than send a token that lapses in flight (clock skew / 401).
    assert!(oauth_token_is_expired(&OAuthTokenSet {
        access_token: "access-token".to_string(),
        refresh_token: None,
        expires_at: Some(now_unix_timestamp() + 30),
        scopes: Vec::new(),
    }));
}

#[test]
fn resolve_saved_oauth_token_refreshes_expired_credentials() {
    let _guard = env_lock();
    let config_home = temp_config_home();
    std::env::set_var("ZO_CONFIG_HOME", &config_home);
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    std::env::remove_var("ANTHROPIC_API_KEY");
    save_oauth_credentials(&core_types::OAuthTokenSet {
        access_token: "expired-access-token".to_string(),
        refresh_token: Some("refresh-token".to_string()),
        expires_at: Some(1),
        scopes: vec!["scope:a".to_string()],
    })
    .expect("save expired oauth credentials");

    let resolved = resolve_saved_oauth_token_set_with(
        &sample_oauth_config("https://console.test/oauth/token".to_string()),
        OAuthTokenSet {
            access_token: "expired-access-token".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            expires_at: Some(1),
            scopes: vec!["scope:a".to_string()],
        },
        |_config, request| {
            assert_eq!(request.refresh_token, "refresh-token");
            Ok(OAuthTokenSet {
                access_token: "refreshed-token".to_string(),
                refresh_token: Some("fresh-refresh".to_string()),
                expires_at: Some(9_999_999_999),
                scopes: request.scopes,
            })
        },
    )
    .expect("resolve refreshed token");
    assert_eq!(resolved.access_token, "refreshed-token");
    let stored = crate::oauth_store::load_oauth_credentials()
        .expect("load stored credentials")
        .expect("stored token set");
    assert_eq!(stored.access_token, "refreshed-token");

    clear_oauth_credentials().expect("clear credentials");
    std::env::remove_var("ZO_CONFIG_HOME");
    cleanup_temp_config_home(&config_home);
}

/// A refresh token the endpoint has already rejected can never mint another
/// access token — the rotating family moved on. Retrying it spent a doomed
/// round-trip on every credential resolution, i.e. once per turn for the whole
/// session, and each one printed the same unactionable 400.
#[test]
fn a_rejected_refresh_branch_is_not_spent_again() {
    let _guard = env_lock();
    let config_home = temp_config_home();
    std::env::set_var("ZO_CONFIG_HOME", &config_home);
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    std::env::remove_var("ANTHROPIC_API_KEY");
    let config = sample_oauth_config("https://console.test/oauth/token".to_string());
    let expired = || OAuthTokenSet {
        access_token: "expired-access-token".to_string(),
        refresh_token: Some("superseded-branch".to_string()),
        expires_at: Some(1),
        scopes: vec!["scope:a".to_string()],
    };
    let invalid_grant = || ApiError::Api {
        status: reqwest::StatusCode::BAD_REQUEST,
        error_type: None,
        message: None,
        body: r#"{"error": "invalid_grant", "error_description": "Refresh token not found or invalid"}"#
            .to_string(),
        retryable: false,
        retry_after: None,
    };

    let first =
        resolve_saved_oauth_token_set_with(&config, expired(), |_config, _request| {
            Err(invalid_grant())
        })
        .expect_err("a rejected grant fails the resolution");
    assert!(
        matches!(first, ApiError::Api { status, .. } if status.as_u16() == 400),
        "the first attempt surfaces the endpoint's own answer: {first}"
    );

    // Second attempt: the closure must never run.
    let second = resolve_saved_oauth_token_set_with(&config, expired(), |_config, _request| {
        panic!("a retired branch must not reach the token endpoint again")
    })
    .expect_err("a retired branch still fails, just without the round-trip");
    assert!(
        matches!(&second, ApiError::Auth(message) if message.contains("zo login claude")),
        "the failure has to name the way out: {second}"
    );

    // A different token is a different branch: a fresh sign-in is never blocked
    // by the death of the one it replaced.
    let fresh = OAuthTokenSet {
        refresh_token: Some("freshly-signed-in".to_string()),
        ..expired()
    };
    let resolved = resolve_saved_oauth_token_set_with(&config, fresh, |_config, request| {
        assert_eq!(request.refresh_token, "freshly-signed-in");
        Ok(OAuthTokenSet {
            access_token: "new-access-token".to_string(),
            refresh_token: Some("rotated".to_string()),
            expires_at: Some(9_999_999_999),
            scopes: request.scopes,
        })
    })
    .expect("an unrelated branch refreshes normally");
    assert_eq!(resolved.access_token, "new-access-token");

    clear_oauth_credentials().expect("clear credentials");
    std::env::remove_var("ZO_CONFIG_HOME");
    cleanup_temp_config_home(&config_home);
}

#[test]
fn resolve_startup_auth_source_uses_saved_oauth_without_loading_config() {
    let _guard = env_lock();
    let config_home = temp_config_home();
    std::env::set_var("ZO_CONFIG_HOME", &config_home);
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    std::env::remove_var("ANTHROPIC_API_KEY");
    save_oauth_credentials(&core_types::OAuthTokenSet {
        access_token: "saved-access-token".to_string(),
        refresh_token: Some("refresh".to_string()),
        expires_at: Some(now_unix_timestamp() + 300),
        scopes: vec!["scope:a".to_string()],
    })
    .expect("save oauth credentials");

    let auth = resolve_startup_auth_source(|| panic!("config should not be loaded"))
        .expect("startup auth");
    assert_eq!(auth.bearer_token(), Some("saved-access-token"));

    clear_oauth_credentials().expect("clear credentials");
    std::env::remove_var("ZO_CONFIG_HOME");
    cleanup_temp_config_home(&config_home);
}

#[test]
fn resolve_startup_auth_source_errors_when_refreshable_token_lacks_config() {
    let _guard = env_lock();
    let config_home = temp_config_home();
    std::env::set_var("ZO_CONFIG_HOME", &config_home);
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    std::env::remove_var("ANTHROPIC_API_KEY");
    save_oauth_credentials(&core_types::OAuthTokenSet {
        access_token: "expired-access-token".to_string(),
        refresh_token: Some("refresh-token".to_string()),
        expires_at: Some(1),
        scopes: vec!["scope:a".to_string()],
    })
    .expect("save expired oauth credentials");

    let error = resolve_startup_auth_source(|| Ok(None)).expect_err("missing config should error");
    assert!(
        matches!(error, crate::error::ApiError::Auth(message) if message.contains("runtime OAuth config is missing"))
    );

    let stored = crate::oauth_store::load_oauth_credentials()
        .expect("load stored credentials")
        .expect("stored token set");
    assert_eq!(stored.access_token, "expired-access-token");
    assert_eq!(stored.refresh_token.as_deref(), Some("refresh-token"));

    clear_oauth_credentials().expect("clear credentials");
    std::env::remove_var("ZO_CONFIG_HOME");
    cleanup_temp_config_home(&config_home);
}

#[test]
fn resolve_saved_oauth_token_preserves_refresh_token_when_refresh_response_omits_it() {
    let _guard = env_lock();
    let config_home = temp_config_home();
    std::env::set_var("ZO_CONFIG_HOME", &config_home);
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    std::env::remove_var("ANTHROPIC_API_KEY");
    save_oauth_credentials(&core_types::OAuthTokenSet {
        access_token: "expired-access-token".to_string(),
        refresh_token: Some("refresh-token".to_string()),
        expires_at: Some(1),
        scopes: vec!["scope:a".to_string()],
    })
    .expect("save expired oauth credentials");

    let resolved = resolve_saved_oauth_token_set_with(
        &sample_oauth_config("https://console.test/oauth/token".to_string()),
        OAuthTokenSet {
            access_token: "expired-access-token".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            expires_at: Some(1),
            scopes: vec!["scope:a".to_string()],
        },
        |_config, request| {
            assert_eq!(request.refresh_token, "refresh-token");
            Ok(OAuthTokenSet {
                access_token: "refreshed-token".to_string(),
                refresh_token: None,
                expires_at: Some(9_999_999_999),
                scopes: request.scopes,
            })
        },
    )
    .expect("resolve refreshed token");
    assert_eq!(resolved.access_token, "refreshed-token");
    assert_eq!(resolved.refresh_token.as_deref(), Some("refresh-token"));
    let stored = crate::oauth_store::load_oauth_credentials()
        .expect("load stored credentials")
        .expect("stored token set");
    assert_eq!(stored.refresh_token.as_deref(), Some("refresh-token"));

    clear_oauth_credentials().expect("clear credentials");
    std::env::remove_var("ZO_CONFIG_HOME");
    cleanup_temp_config_home(&config_home);
}

#[test]
fn message_request_stream_helper_sets_stream_true() {
    let request = MessageRequest {
        model: "claude-opus-4-6".to_string(),
        max_tokens: 64,
        messages: vec![],
        system: None,
        tools: None,
        tool_choice: None,
        stream: false,
        thinking: None,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    };

    assert!(request.with_streaming().stream);
}

#[test]
fn backoff_doubles_until_maximum() {
    let client = AnthropicClient::new("test-key").with_retry_policy(
        3,
        Duration::from_millis(100),
        Duration::from_millis(250),
    );
    // Jitter band is [0.5x, 1.5x), so a 100ms base falls in [50ms, 150ms),
    // 200ms in [100ms, 300ms), and 250ms (max-capped) in [125ms, 375ms).
    // Sample many times — a single draw can land anywhere in the band.
    for _ in 0..32 {
        let a1 = client.backoff_for_attempt(1).expect("attempt 1");
        assert!(
            a1 >= Duration::from_millis(50) && a1 < Duration::from_millis(150),
            "attempt 1 out of band: {a1:?}"
        );
        let a2 = client.backoff_for_attempt(2).expect("attempt 2");
        assert!(
            a2 >= Duration::from_millis(100) && a2 < Duration::from_millis(300),
            "attempt 2 out of band: {a2:?}"
        );
        let a3 = client.backoff_for_attempt(3).expect("attempt 3");
        assert!(
            a3 >= Duration::from_millis(125) && a3 < Duration::from_millis(375),
            "attempt 3 out of band: {a3:?}"
        );
    }
}

#[test]
fn retryable_statuses_are_detected() {
    assert!(super::is_retryable_status(
        reqwest::StatusCode::TOO_MANY_REQUESTS
    ));
    assert!(super::is_retryable_status(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    ));
    // 529 `overloaded_error` is Anthropic's transient overload signal and MUST
    // be retried like the official SDK does — the old fixed 5xx whitelist
    // dropped it, surfacing the overload to the user.
    assert!(super::is_retryable_status(
        reqwest::StatusCode::from_u16(529).unwrap()
    ));
    // Other 5xx the old whitelist omitted are now retryable too.
    assert!(super::is_retryable_status(
        reqwest::StatusCode::NOT_IMPLEMENTED // 501
    ));
    assert!(super::is_retryable_status(
        reqwest::StatusCode::REQUEST_TIMEOUT // 408
    ));
    assert!(super::is_retryable_status(reqwest::StatusCode::CONFLICT)); // 409
    // Client errors other than 408/409/429 must still fail fast.
    assert!(!super::is_retryable_status(
        reqwest::StatusCode::UNAUTHORIZED
    ));
    assert!(!super::is_retryable_status(
        reqwest::StatusCode::BAD_REQUEST
    ));
}

#[test]
fn tool_delta_variant_round_trips() {
    let delta = ContentBlockDelta::InputJsonDelta {
        partial_json: "{\"city\":\"Paris\"}".to_string(),
    };
    let encoded = serde_json::to_string(&delta).expect("delta should serialize");
    let decoded: ContentBlockDelta =
        serde_json::from_str(&encoded).expect("delta should deserialize");
    assert_eq!(decoded, delta);
}

#[test]
fn request_id_uses_primary_or_fallback_header() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(REQUEST_ID_HEADER, "req_primary".parse().expect("header"));
    assert_eq!(
        super::request_id_from_headers(&headers).as_deref(),
        Some("req_primary")
    );

    headers.clear();
    headers.insert(
        ALT_REQUEST_ID_HEADER,
        "req_fallback".parse().expect("header"),
    );
    assert_eq!(
        super::request_id_from_headers(&headers).as_deref(),
        Some("req_fallback")
    );
}

#[test]
fn auth_source_applies_headers() {
    let auth = AuthSource::ApiKeyAndBearer {
        api_key: "test-key".to_string(),
        bearer_token: "proxy-token".to_string(),
    };
    let request = auth
        .apply(reqwest::Client::new().post("https://example.test"))
        .build()
        .expect("request build");
    let headers = request.headers();
    assert_eq!(
        headers.get("x-api-key").and_then(|v| v.to_str().ok()),
        Some("test-key")
    );
    assert_eq!(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some("Bearer proxy-token")
    );
}

#[test]
fn parse_rfc3339_to_unix_known_values() {
    use super::parse_rfc3339_to_unix;
    assert_eq!(parse_rfc3339_to_unix("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        parse_rfc3339_to_unix("2000-01-01T00:00:00Z"),
        Some(946_684_800)
    );
    assert_eq!(parse_rfc3339_to_unix("nonsense"), None);
}

#[test]
fn parse_reset_accepts_unix_and_rfc3339() {
    use super::parse_reset_to_unix;
    assert_eq!(parse_reset_to_unix("1769904000"), Some(1_769_904_000));
    assert_eq!(
        parse_reset_to_unix("2000-01-01T00:00:00Z"),
        Some(946_684_800)
    );
}

#[test]
fn ratelimit_from_headers_parses_unified_windows() {
    use super::ratelimit_from_headers;
    use core_types::RateLimitWindowKind;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "anthropic-ratelimit-unified-5h-utilization",
        "0.68".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-unified-7d-utilization",
        "0.41".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-unified-representative-claim",
        "five_hour".parse().unwrap(),
    );
    let snap = ratelimit_from_headers(&headers).expect("unified headers present");
    assert_eq!(snap.five_hour.unwrap().used_percent(), 68);
    assert_eq!(snap.seven_day.unwrap().used_percent(), 41);
    assert_eq!(snap.representative, Some(RateLimitWindowKind::FiveHour));
}

#[test]
fn ratelimit_from_headers_none_for_api_key() {
    use super::ratelimit_from_headers;
    // API-key responses carry no unified headers → no gauge.
    let headers = reqwest::header::HeaderMap::new();
    assert!(ratelimit_from_headers(&headers).is_none());
}

#[test]
fn set_auth_swaps_bearer_in_place() {
    // Mid-session OAuth refresh path: a long-lived client must adopt a fresh
    // bearer without being rebuilt. set_auth mutates only the auth field.
    let mut client = AnthropicClient::from_auth(AuthSource::BearerToken("stale-token".into()));
    assert_eq!(client.auth_source().bearer_token(), Some("stale-token"));

    client.set_auth(AuthSource::BearerToken("refreshed-token".into()));
    assert_eq!(client.auth_source().bearer_token(), Some("refreshed-token"));
}

#[test]
fn oauth_token_response_translates_scope_string_and_expires_in() {
    // The real Anthropic token endpoint returns `scope` (space-separated) and
    // `expires_in` (relative seconds). Before the shim these were dropped and
    // the saved token had empty scopes / null expiry — the root cause of the
    // 403 `does not meet scope requirement` after `zo login`.
    let raw: super::OAuthTokenResponse = serde_json::from_str(
        r#"{"access_token":"at","refresh_token":"rt","expires_in":3600,"scope":"user:profile user:inference"}"#,
    )
    .expect("token response should parse");
    let token = raw.into_token_set(1000);
    assert_eq!(token.access_token, "at");
    assert_eq!(token.refresh_token.as_deref(), Some("rt"));
    assert_eq!(token.expires_at, Some(4600));
    assert_eq!(
        token.scopes,
        vec!["user:profile".to_string(), "user:inference".to_string()]
    );
}

#[test]
fn oauth_token_response_prefers_absolute_expiry_and_array_scopes() {
    // Defensive: if the server ever sends an absolute `expires_at` or an array
    // `scopes`, honor those over the relative/string forms.
    let raw: super::OAuthTokenResponse = serde_json::from_str(
        r#"{"access_token":"at","expires_at":9999,"expires_in":10,"scopes":["a","b"],"scope":"ignored"}"#,
    )
    .expect("token response should parse");
    let token = raw.into_token_set(1000);
    assert_eq!(token.expires_at, Some(9999));
    assert_eq!(token.scopes, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn oauth_token_response_handles_missing_optional_fields() {
    let raw: super::OAuthTokenResponse =
        serde_json::from_str(r#"{"access_token":"only"}"#).expect("token response should parse");
    let token = raw.into_token_set(1000);
    assert_eq!(token.access_token, "only");
    assert_eq!(token.refresh_token, None);
    assert_eq!(token.expires_at, None);
    assert!(token.scopes.is_empty());
}

// ---------------------------------------------------------------------------
// Mid-stream restart (L1): a pre-commit stall transparently re-opens the
// request; a post-commit stall must propagate (no duplicate output).
// ---------------------------------------------------------------------------

/// Env override key for the per-chunk idle budget (shared across providers).
const IDLE_ENV: &str = super::super::STREAM_IDLE_TIMEOUT_ENV;

fn streaming_request() -> MessageRequest {
    MessageRequest {
        model: "claude-opus-4-6".to_string(),
        max_tokens: 64,
        messages: vec![InputMessage::user_text("hi")],
        system: None,
        tools: None,
        tool_choice: None,
        stream: false,
        thinking: None,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    }
}

#[tokio::test]
async fn unauthenticated_client_warmup_does_not_touch_network() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local listener");
    let addr = listener.local_addr().expect("listener addr");
    let client = AnthropicClient::from_auth(AuthSource::None).with_base_url(format!("http://{addr}"));

    client.warm_connection().await;

    let accepted = tokio::time::timeout(Duration::from_millis(50), listener.accept()).await;
    assert!(accepted.is_err(), "unauthenticated warmup opened a connection");
}



#[tokio::test]
async fn unauthenticated_client_blocks_send_locally_before_external_network() {
    let request = streaming_request();
    let client = AnthropicClient::from_auth(AuthSource::None)
        .with_base_url("https://api.anthropic.com");

    let error = client
        .send_message(&request)
        .await
        .expect_err("missing auth should fail locally");

    match error {
        ApiError::Auth(message) => assert!(message.contains("/login claude")),
        other => panic!("expected local auth error, got {other:?}"),
    }
}

#[tokio::test]
async fn unauthenticated_client_blocks_stream_locally_before_external_network() {
    let request = streaming_request();
    let client = AnthropicClient::from_auth(AuthSource::None)
        .with_base_url("https://api.anthropic.com");

    let error = client
        .stream_message(&request)
        .await
        .expect_err("missing auth should fail locally");

    match error {
        ApiError::Auth(message) => assert!(message.contains("/login claude")),
        other => panic!("expected local auth error, got {other:?}"),
    }
}

/// Foreground visibility regression: rate-limit retries can happen while
/// establishing the stream, before a `MessageStream` exists. The retry notice
/// callback fires immediately before the sleep so the TUI can render
/// "rate limited; retrying..." instead of looking frozen.
#[tokio::test]
async fn retry_notice_fires_before_stream_establish_sleep() {
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // First request: 429 with a tiny Retry-After so the test is fast.
        let (mut first, _) = listener.accept().await.unwrap();
        let mut scratch = [0u8; 2048];
        let _ = first.read(&mut scratch).await;
        let first_body = b"{\"error\":{\"type\":\"rate_limit_error\",\"message\":\"slow down\"},\"type\":\"error\"}";
        let first_head = format!(
            "HTTP/1.1 429 Too Many Requests\r\nconnection: close\r\nretry-after: 0\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            first_body.len()
        );
        first.write_all(first_head.as_bytes()).await.unwrap();
        first.write_all(first_body).await.unwrap();
        first.flush().await.unwrap();
        first.shutdown().await.unwrap();

        // Second request: open a minimal successful stream.
        let (mut second, _) = listener.accept().await.unwrap();
        let _ = second.read(&mut scratch).await;
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-4-6\"}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        second.shutdown().await.unwrap();
    });

    let notices = Arc::new(Mutex::new(Vec::new()));
    let seen = notices.clone();
    let client = AnthropicClient::new("token")
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(1, Duration::from_millis(1), Duration::from_millis(5))
        .with_retry_notice_callback(move |notice| {
            seen.lock().unwrap().push((
                notice.attempt,
                notice.max_attempts,
                core_types::retry_signal::retry_notice_label(&notice.error),
                notice.delay,
            ));
        });

    let stream = client
        .stream_message(&streaming_request())
        .await
        .expect("second request succeeds");
    drop(stream);
    server.await.unwrap();

    let notices = notices.lock().unwrap();
    assert_eq!(notices.len(), 1, "one 429 retry notice");
    assert_eq!(notices[0].0, 1);
    assert_eq!(notices[0].1, 2);
    // The notice carries the flattened error and the renderer labels it through
    // the shared classifier, so the row can name the exact wall — a 429 here, a
    // "provider overloaded" for a 529 — instead of the old boolean's two buckets.
    assert_eq!(notices[0].2, "rate limited");
}

#[tokio::test]
async fn fallback_ready_client_returns_first_rate_limit_without_inner_retry() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let server_hits = hits.clone();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, Ordering::SeqCst);
        let mut scratch = [0u8; 2048];
        let _ = first.read(&mut scratch).await;
        let body = b"{\"error\":{\"type\":\"rate_limit_error\",\"message\":\"slow down\"},\"type\":\"error\"}";
        let head = format!(
            "HTTP/1.1 429 Too Many Requests\r\nconnection: close\r\nretry-after: 0\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        first.write_all(head.as_bytes()).await.unwrap();
        first.write_all(body).await.unwrap();
        first.flush().await.unwrap();
        first.shutdown().await.unwrap();

        // A broken fail-fast policy reaches this second request and succeeds,
        // making the test fail sharply instead of hanging on an empty listener.
        if let Ok(Ok((mut second, _))) = tokio::time::timeout(
            Duration::from_millis(200),
            listener.accept(),
        )
        .await
        {
            server_hits.fetch_add(1, Ordering::SeqCst);
            let _ = second.read(&mut scratch).await;
            let stream_body = concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-4-6\"}}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n",
            );
            let stream_head = format!(
                "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
                stream_body.len()
            );
            second.write_all(stream_head.as_bytes()).await.unwrap();
            second.write_all(stream_body.as_bytes()).await.unwrap();
            second.flush().await.unwrap();
        }
    });

    let client = AnthropicClient::new("token")
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(1, Duration::from_millis(1), Duration::from_millis(5))
        .with_rate_limit_fail_fast();
    let error = client
        .stream_message(&streaming_request())
        .await
        .expect_err("fallback-ready client must expose the first 429");
    server.await.unwrap();

    assert!(error.is_rate_limit());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// RAII env-var guard: restores the prior value on drop, even on a panicking
/// `.await`/assertion, so a failing test can never leak `IDLE_ENV` into the
/// next env test (`stream_idle_timeout_defaults_and_env_override`, which reads
/// the same var). Mirrors the per-test-module guards in `mod.rs`/`client.rs`.
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
fn claude_config_credentials_are_reported_as_a_managed_file() {
    let _guard = env_lock();
    let _isolation = crate::test_env::CredentialEnvIsolation::empty();
    let managed = tempfile::tempdir().expect("managed Claude home");
    std::fs::write(
        managed.path().join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"managed-token","scopes":["user:inference"]}}"#,
    )
    .expect("managed credentials");
    let _claude_home = EnvVarGuard::set(
        "CLAUDE_CONFIG_DIR",
        Some(managed.path().to_str().expect("utf8 managed home")),
    );
    let _disable_keychain = EnvVarGuard::set("ZO_DISABLE_KEYCHAIN", Some("1"));
    super::keychain::invalidate_claude_code_keychain_cache();

    let resolved = super::resolve_claude_auth_fresh_detailed().expect("managed auth");

    assert_eq!(resolved.origin, super::ClaudeAuthOrigin::ManagedFile);
    assert_eq!(resolved.auth.bearer_token(), Some("managed-token"));
}

struct ContextEditTestGuard {
    _env: EnvVarGuard,
}

impl ContextEditTestGuard {
    fn new() -> Self {
        super::CONTEXT_EDIT_SURFACE_UNSUPPORTED
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Self {
            _env: EnvVarGuard::set("ZO_ANTHROPIC_CONTEXT_EDIT", Some("1")),
        }
    }
}

impl Drop for ContextEditTestGuard {
    fn drop(&mut self) {
        super::CONTEXT_EDIT_SURFACE_UNSUPPORTED
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;

    let mut request = Vec::new();
    let mut scratch = [0u8; 2048];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut scratch))
            .await
            .expect("request read timed out")
            .expect("read request");
        assert!(read > 0, "connection closed before request completed");
        request.extend_from_slice(&scratch[..read]);

        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&request[..header_end]).expect("request headers utf8");
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content-length"))
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            return String::from_utf8(request).expect("request utf8");
        }
    }
}

async fn write_json_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    body: &str,
) {
    use tokio::io::AsyncWriteExt;

    let head = format!(
        "HTTP/1.1 {status}\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.expect("write response head");
    stream.write_all(body.as_bytes()).await.expect("write response body");
    stream.flush().await.expect("flush response");
    stream.shutdown().await.expect("shutdown response");
}

fn request_context_edit_state(request: &str) -> (bool, bool) {
    let (headers, body) = request
        .split_once("\r\n\r\n")
        .expect("complete HTTP request");
    let beta_present = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .any(|(name, value)| {
            name.eq_ignore_ascii_case("anthropic-beta")
                && value.contains("context-management-2025-06-27")
        });
    let body: serde_json::Value = serde_json::from_str(body).expect("request JSON body");
    (beta_present, body.get("context_management").is_some())
}

const CONTEXT_EDIT_UNSUPPORTED_BODY: &str = concat!(
    "{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",",
    "\"message\":\"unsupported anthropic-beta: context-management-2025-06-27\"}}"
);
const CONTEXT_EDIT_SUCCESS_BODY: &str = concat!(
    "{\"id\":\"msg_context_edit\",\"type\":\"message\",\"role\":\"assistant\",",
    "\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],",
    "\"model\":\"claude-opus-4-8\",\"stop_reason\":\"end_turn\",",
    "\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},",
    "\"context_management\":{\"applied_edits\":[{\"type\":\"clear_tool_uses_20250919\",",
    "\"cleared_tool_uses\":4,\"cleared_input_tokens\":22000}]}}"
);

/// A surface that rejects context editing must receive one immediate retry of
/// the same request, with both the beta and body parameter removed on the wire.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn context_edit_400_retries_without_beta_or_body() {
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    let _env_guard = env_lock();
    let _context_edit_guard = ContextEditTestGuard::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("listener addr");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let server_captured = Arc::clone(&captured);

    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.expect("first request");
        server_captured.lock().await.push(read_http_request(&mut first).await);
        write_json_response(&mut first, "400 Bad Request", CONTEXT_EDIT_UNSUPPORTED_BODY).await;

        let (mut second, _) = listener.accept().await.expect("fallback request");
        server_captured.lock().await.push(read_http_request(&mut second).await);
        write_json_response(&mut second, "200 OK", CONTEXT_EDIT_SUCCESS_BODY).await;
    });

    let client = AnthropicClient::new("token")
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(0, Duration::from_millis(1), Duration::from_millis(1))
        .with_env_context_editing();
    let response = client
        .send_message(&streaming_request())
        .await
        .expect("fallback request succeeds");
    assert_eq!(response.id, "msg_context_edit");
    let context_management = response
        .context_management
        .as_ref()
        .expect("applied edits are preserved");
    assert_eq!(context_management.cleared_tool_uses(), 4);
    assert_eq!(context_management.cleared_input_tokens(), 22_000);
    let analytics = super::context_edit_analytics_event(
        response.request_id.as_deref(),
        response.context_management.as_ref(),
    )
    .expect("applied edits emit telemetry");
    assert_eq!(analytics.namespace, "api");
    assert_eq!(analytics.action, "context_edit_applied");
    assert_eq!(analytics.properties["applied_edit_count"], serde_json::json!(1));
    assert_eq!(analytics.properties["cleared_tool_uses"], serde_json::json!(4));
    assert_eq!(
        analytics.properties["cleared_input_tokens"],
        serde_json::json!(22_000)
    );
    server.await.expect("server task");

    let captured = captured.lock().await;
    assert_eq!(captured.len(), 2);
    assert_eq!(request_context_edit_state(&captured[0]), (true, true));
    assert_eq!(request_context_edit_state(&captured[1]), (false, false));
}

/// Both in-flight requests carried the edit, so both context-edit 400s must
/// retry even after the first response has already lowered the global latch.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn concurrent_context_edit_400s_each_retry_without_edit() {
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    let _env_guard = env_lock();
    let _context_edit_guard = ContextEditTestGuard::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("listener addr");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let server_captured = Arc::clone(&captured);

    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.expect("first initial request");
        let first_request = read_http_request(&mut first).await;
        let (mut second, _) = listener.accept().await.expect("second initial request");
        let second_request = read_http_request(&mut second).await;
        server_captured.lock().await.extend([first_request, second_request]);

        write_json_response(&mut first, "400 Bad Request", CONTEXT_EDIT_UNSUPPORTED_BODY).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while !super::CONTEXT_EDIT_SURFACE_UNSUPPORTED
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first 400 did not lower context-edit latch");
        write_json_response(&mut second, "400 Bad Request", CONTEXT_EDIT_UNSUPPORTED_BODY).await;

        for _ in 0..2 {
            let Ok(Ok((mut retry, _))) =
                tokio::time::timeout(Duration::from_secs(1), listener.accept()).await
            else {
                break;
            };
            server_captured.lock().await.push(read_http_request(&mut retry).await);
            write_json_response(&mut retry, "200 OK", CONTEXT_EDIT_SUCCESS_BODY).await;
        }
    });

    let client = AnthropicClient::new("token")
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(0, Duration::from_millis(1), Duration::from_millis(1))
        .with_env_context_editing();
    let request = streaming_request();
    let (first, second) = tokio::join!(client.send_message(&request), client.send_message(&request));
    server.await.expect("server task");

    assert!(first.is_ok(), "first request failed: {first:?}");
    assert!(second.is_ok(), "second request failed: {second:?}");
    let captured = captured.lock().await;
    assert_eq!(captured.len(), 4, "both initial requests must retry");
    assert!(captured[..2]
        .iter()
        .all(|request| request_context_edit_state(request) == (true, true)));
    assert!(captured[2..]
        .iter()
        .all(|request| request_context_edit_state(request) == (false, false)));
}

/// An EOF-terminated final SSE frame must take the same observation path as a
/// normally delimited frame so usage and context-edit telemetry are not lost.
#[tokio::test]
async fn eof_buffered_message_delta_is_observed_and_traced() {
    use std::sync::Arc;
    use telemetry::{MemoryTelemetrySink, SessionTracer, TelemetryEvent};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("listener addr");
    let server = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.expect("stream request");
        let _ = read_http_request(&mut connection).await;
        let body = concat!(
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",",
            "\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},",
            "\"usage\":{\"input_tokens\":7,\"output_tokens\":3},",
            "\"context_management\":{\"applied_edits\":[{",
            "\"type\":\"clear_tool_uses_20250919\",",
            "\"cleared_tool_uses\":2,\"cleared_input_tokens\":21000}]}}"
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        connection.write_all(head.as_bytes()).await.expect("write head");
        connection.write_all(body.as_bytes()).await.expect("write body");
        connection.shutdown().await.expect("close connection");
    });

    let sink = Arc::new(MemoryTelemetrySink::default());
    let tracer = SessionTracer::new("context-edit-eof", sink.clone());
    let client = AnthropicClient::new("token")
        .with_base_url(format!("http://{addr}"))
        .with_session_tracer(tracer);
    let request = streaming_request();
    let mut stream = client.stream_message(&request).await.expect("open stream");

    let event = stream
        .next_event()
        .await
        .expect("read final frame")
        .expect("message delta");
    let StreamEvent::MessageDelta(delta) = event else {
        panic!("expected message delta");
    };
    let context_management = delta.context_management.expect("applied edits");
    assert_eq!(context_management.cleared_input_tokens(), 21_000);
    assert_eq!(
        stream.latest_usage,
        Some(crate::types::Usage {
            input_tokens: 7,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            output_tokens: 3,
            output_tokens_details: None,
        }),
        "EOF frame must pass through observe_event"
    );
    assert!(stream.next_event().await.expect("clean EOF").is_none());
    server.await.expect("server task");

    let applied: Vec<_> = sink
        .events()
        .into_iter()
        .filter_map(|event| match event {
            TelemetryEvent::Analytics(event) if event.action == "context_edit_applied" => {
                Some(event)
            }
            _ => None,
        })
        .collect();
    assert_eq!(applied.len(), 1);
    assert_eq!(
        applied[0].properties["cleared_input_tokens"],
        serde_json::json!(21_000)
    );
}

/// End-to-end proof that a pre-commit stall recovers over a real socket: the
/// mock server's first connection sends only headers then goes silent, and the
/// second serves a full SSE turn. With a sub-second idle budget the stream must
/// idle out, re-open the request, and yield the recovered text — once, no error.
// `#[tokio::test]` is single-threaded, so holding the env lock across `.await`
// only serialises the process-global idle-timeout var against other env tests —
// it cannot deadlock the executor.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn stalled_precommit_stream_restarts_and_recovers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        // Connection 1: headers, then stall (no body) → forces a pre-commit idle
        // timeout. Held open long enough to outlast the 300ms idle budget.
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 1024];
        let _ = first.read(&mut scratch).await;
        first
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        first.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;

        // Connection 2 (the restart): a complete SSE turn with content-length so
        // the body ends cleanly.
        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        let body = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"recovered\"}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    });

    let _idle = EnvVarGuard::set(IDLE_ENV, Some("300"));
    let client = AnthropicClient::new("token")
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(3, Duration::from_millis(10), Duration::from_millis(50));

    let request = streaming_request();
    let mut stream = client.stream_message(&request).await.expect("open stream");

    let mut text = String::new();
    while let Some(event) = stream.next_event().await.expect("no error after restart") {
        if let StreamEvent::ContentBlockDelta(delta) = &event {
            if let ContentBlockDelta::TextDelta { text: chunk } = &delta.delta {
                text.push_str(chunk);
            }
        }
    }
    server.await.unwrap();

    assert_eq!(
        text, "recovered",
        "recovered turn must stream after restart"
    );
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "server hit exactly twice: the stalled attempt + the restart"
    );
}

/// The duplication-safety guarantee: once a non-empty text delta has been
/// surfaced (`committed`), a later idle stall must propagate as an error rather
/// than re-open the request (which would replay the already-seen text). The
/// server therefore sees exactly one connection.
// Single-threaded test; the env lock across `.await` only serialises env access.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn committed_stream_propagates_instead_of_restarting() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        // One connection only: headers + a real text delta (commits the turn),
        // then stall. A restart here would open a second connection — it must not.
        let (mut conn, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 1024];
        let _ = conn.read(&mut scratch).await;
        conn.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        conn.write_all(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        )
        .await
        .unwrap();
        conn.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    });

    let _idle = EnvVarGuard::set(IDLE_ENV, Some("300"));
    let client = AnthropicClient::new("token")
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(3, Duration::from_millis(10), Duration::from_millis(50));

    let request = streaming_request();
    let mut stream = client.stream_message(&request).await.expect("open stream");

    // First event is the committing text delta.
    let first = stream
        .next_event()
        .await
        .expect("first event ok")
        .expect("a text delta");
    match first {
        StreamEvent::ContentBlockDelta(delta) => match delta.delta {
            ContentBlockDelta::TextDelta { text } => assert_eq!(text, "partial"),
            other => panic!("expected text delta, got {other:?}"),
        },
        other => panic!("expected content_block_delta, got {other:?}"),
    }
    // The post-commit stall must surface as the idle-timeout error itself, not a
    // silent restart and not some other fault — pinning the source guards against
    // a regression where the restart gate mis-fires and a different error leaks.
    let second = stream.next_event().await;
    assert!(
        matches!(
            &second,
            Err(crate::error::ApiError::StreamApi { error_type, .. })
                if error_type.as_deref() == Some("stream_idle_timeout")
        ),
        "post-commit idle must propagate as stream_idle_timeout, got {second:?}"
    );
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "committed turn must not re-open a second connection"
    );
    server.abort();
}

// --- Wire thinking/effort normalization (adaptive vs legacy) ---------------

use super::normalize_thinking_for_wire;
use crate::types::{OutputConfig, ThinkingConfig};

fn req(model: &str, thinking: Option<ThinkingConfig>) -> MessageRequest {
    MessageRequest {
        model: model.to_string(),
        max_tokens: 1000,
        messages: vec![InputMessage::user_text("hi")],
        system: None,
        tools: None,
        tool_choice: None,
        stream: true,
        thinking,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    }
}

#[test]
fn adaptive_model_translates_budget_to_output_config_effort() {
    // An Opus 4.8 request carrying the 16k Xhigh-preset budget must go out as
    // output_config.effort + thinking:{type:"adaptive"}, with NO budget_tokens.
    // 16k is the CLI `Effort::Xhigh` budget, so it must reach the wire as
    // `xhigh` — not a tier low (the under-clocking bug this maps fixes).
    let request = req("claude-opus-4-8", Some(ThinkingConfig::enabled(16_000)));
    let normalized = normalize_thinking_for_wire(&request);

    let oc = normalized.output_config.as_ref().expect("effort set");
    assert_eq!(oc.effort.anthropic(), "xhigh"); // 16k Xhigh preset → xhigh
    let thinking = normalized.thinking.as_ref().expect("thinking kept");
    assert_eq!(thinking.kind, "adaptive");
    assert!(
        thinking.budget_tokens.is_none(),
        "no deprecated budget on wire"
    );
    // Opus opts into a *visible* summarized reasoning stream so a long thinking
    // pass shows live progress instead of the default `omitted` empty blocks.
    assert_eq!(thinking.display.as_deref(), Some("summarized"));

    // And the serialized JSON proves the wire shape.
    let body = serde_json::to_value(normalized.as_ref()).unwrap();
    assert_eq!(body["output_config"]["effort"], "xhigh");
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert!(body["thinking"].get("budget_tokens").is_none());
    assert_eq!(body["thinking"]["display"], "summarized");
}

#[test]
fn max_preset_budget_reaches_wire_as_max_effort() {
    // The headline benchmark fix: a headless `ZO_EFFORT=max` threads the 24k
    // Max-preset budget into the request, and on an adaptive Opus model it must
    // reach the wire as output_config.effort="max" — not "high" (the old
    // under-clocking bug that lost the four-way benchmark).
    let request = req("claude-opus-4-8", Some(ThinkingConfig::enabled(24_000)));
    let normalized = normalize_thinking_for_wire(&request);
    let oc = normalized.output_config.as_ref().expect("effort set");
    assert_eq!(oc.effort.anthropic(), "max");
}

use super::strip_thinking_blocks_when_disabled;

fn assistant_with_thinking() -> InputMessage {
    InputMessage {
        role: "assistant".to_string(),
        content: vec![
            crate::types::InputContentBlock::Thinking {
                thinking: "reasoning".to_string(),
                signature: "sig".to_string(),
            },
            crate::types::InputContentBlock::Text {
                text: "answer".to_string(),
                cache_control: None,
            },
        ],
        thought_signature: None,
        reasoning_replay: None,
    }
}

#[test]
fn thinking_blocks_stripped_when_request_has_thinking_disabled() {
    // After `/effort off` the request carries thinking:None, but the replayed
    // history still holds a stored thinking block (convert_blocks lowered it
    // unconditionally). It must be dropped on the wire — a thinking block with no
    // thinking config on the request 400s and then wedges every following turn.
    let mut request = req("claude-opus-4-8", None);
    request.messages.push(assistant_with_thinking());

    let normalized = strip_thinking_blocks_when_disabled(std::borrow::Cow::Borrowed(&request));

    let assistant = normalized.messages.last().expect("assistant message");
    assert!(
        !assistant.content.iter().any(|block| matches!(
            block,
            crate::types::InputContentBlock::Thinking { .. }
                | crate::types::InputContentBlock::RedactedThinking { .. }
        )),
        "thinking block must be stripped when thinking is disabled"
    );
    // The real answer text survives the strip.
    assert!(assistant.content.iter().any(|block| matches!(
        block,
        crate::types::InputContentBlock::Text { text, .. } if text == "answer"
    )));
}

#[test]
fn thinking_blocks_kept_when_thinking_enabled() {
    // With thinking enabled the replayed block is valid (and required before a
    // tool_use), so it must be preserved verbatim and the request left un-cloned.
    let mut request = req("claude-opus-4-8", Some(ThinkingConfig::enabled(16_000)));
    request.messages.push(assistant_with_thinking());

    let normalized = strip_thinking_blocks_when_disabled(std::borrow::Cow::Borrowed(&request));

    let assistant = normalized.messages.last().expect("assistant message");
    assert!(
        assistant.content.iter().any(|block| matches!(
            block,
            crate::types::InputContentBlock::Thinking { signature, .. } if signature == "sig"
        )),
        "thinking block must survive when thinking is enabled"
    );
    assert!(
        matches!(normalized, std::borrow::Cow::Borrowed(_)),
        "no clone when nothing is stripped"
    );
}

#[test]
fn reasoning_only_message_is_dropped_when_thinking_disabled() {
    // A reasoning-only assistant turn (thinking block, no text/tool_use — a
    // max_tokens/refusal edge) would be emptied to `content: []` by the strip.
    // An empty-content message is itself a 400, so it must be dropped entirely,
    // not merely emptied — otherwise one rejection is traded for another and the
    // session stays wedged.
    let mut request = req("claude-opus-4-8", None);
    request.messages.push(InputMessage {
        role: "assistant".to_string(),
        content: vec![crate::types::InputContentBlock::Thinking {
            thinking: "lone reasoning".to_string(),
            signature: "sig".to_string(),
        }],
        thought_signature: None,
        reasoning_replay: None,
    });
    // A redacted-only turn is the same shape (no signature gate).
    request.messages.push(InputMessage {
        role: "assistant".to_string(),
        content: vec![crate::types::InputContentBlock::RedactedThinking {
            data: "ENCRYPTED".to_string(),
        }],
        thought_signature: None,
        reasoning_replay: None,
    });
    let before = request.messages.len();

    let normalized = strip_thinking_blocks_when_disabled(std::borrow::Cow::Borrowed(&request));

    assert_eq!(
        normalized.messages.len(),
        before - 2,
        "both reasoning-only messages must be dropped, not left empty"
    );
    assert!(
        normalized
            .messages
            .iter()
            .all(|message| !message.content.is_empty()),
        "no empty-content message may reach the wire"
    );
}

/// Determinism + idempotency pin for the smart-AUTO cache-collapse
/// investigation: the same request must strip identically every call, and
/// re-running the strip on its OWN already-stripped output must be a true
/// no-op (2nd application == 1st application), which the `Cow::Borrowed`
/// early-return already guarantees once no thinking block remains.
#[test]
fn strip_thinking_blocks_when_disabled_is_deterministic_and_idempotent() {
    let mut request = req("claude-opus-4-8", None);
    request.messages.push(assistant_with_thinking());

    let once = strip_thinking_blocks_when_disabled(std::borrow::Cow::Borrowed(&request));
    let twice_same_input = strip_thinking_blocks_when_disabled(std::borrow::Cow::Borrowed(&request));
    assert_eq!(
        once.as_ref(),
        twice_same_input.as_ref(),
        "same input -> same output (deterministic)"
    );

    // Idempotent: feeding the ALREADY-stripped request back in must change
    // nothing further.
    let stripped_request = once.into_owned();
    let reapplied = strip_thinking_blocks_when_disabled(std::borrow::Cow::Borrowed(&stripped_request));
    assert_eq!(
        reapplied.as_ref(),
        &stripped_request,
        "re-stripping an already-stripped request is a no-op"
    );
    assert!(
        matches!(reapplied, std::borrow::Cow::Borrowed(_)),
        "no clone when there is nothing left to strip"
    );
}

/// RISK PIN — see the task report. The ONLY place that drops a
/// signature=="" thinking block (exactly how GPT-produced reasoning is
/// stored in session history) is `convert_blocks` in
/// `runtime::convert_messages`, far upstream of this function and gated
/// purely on `!signature.is_empty()`, unconditional on whether the
/// downstream request has thinking enabled or disabled. `grep` over the
/// crate confirms `InputContentBlock::Thinking` is constructed in exactly
/// that one place in production code, so in the CURRENT call graph an
/// empty-signature block can never reach `MessageRequest.messages` in the
/// first place — this function never needs to guard it.
///
/// But `strip_thinking_blocks_when_disabled` itself provides no
/// defense-in-depth for that case: it is gated purely on
/// `request.thinking.is_some()` and does nothing at all when thinking is
/// enabled. This test proves that directly — an empty-signature thinking
/// block placed on an enabled-thinking request sails through completely
/// untouched — so if any FUTURE code path ever built `MessageRequest`
/// without going through `convert_messages` (bypassing its filter), this
/// function would forward the block to the Anthropic wire verbatim, which
/// 400s on a modified/unsigned thinking block. Pinning current behavior only;
/// no fix here — deciding whether to add a redundant guard is a cross-model
/// policy call.
#[test]
fn strip_thinking_blocks_when_disabled_does_not_guard_empty_signature_blocks_when_thinking_is_enabled()
 {
    let mut request = req("claude-opus-4-8", Some(ThinkingConfig::enabled(16_000)));
    request.messages.push(InputMessage {
        role: "assistant".to_string(),
        content: vec![
            crate::types::InputContentBlock::Thinking {
                thinking: "gpt reasoning replayed on anthropic".to_string(),
                signature: String::new(),
            },
            crate::types::InputContentBlock::Text {
                text: "answer".to_string(),
                cache_control: None,
            },
        ],
        thought_signature: None,
        reasoning_replay: None,
    });

    let normalized = strip_thinking_blocks_when_disabled(std::borrow::Cow::Borrowed(&request));

    assert!(
        matches!(normalized, std::borrow::Cow::Borrowed(_)),
        "thinking enabled -> function is a pure no-op, no defense-in-depth here"
    );
    let assistant = normalized.messages.last().expect("assistant message");
    assert!(
        assistant.content.iter().any(|block| matches!(
            block,
            crate::types::InputContentBlock::Thinking { signature, .. } if signature.is_empty()
        )),
        "an empty-signature thinking block is NOT stripped when thinking is enabled — \
         it would reach the wire verbatim if it ever got this far"
    );
}

#[test]
fn adaptive_sonnet_clamps_xhigh_budget_to_high_on_wire() {
    // Live-repro regression: a sub-agent demoted opus→sonnet (starvation gate)
    // while carrying the 16k Xhigh preset budget sent output_config.effort=
    // "xhigh", which Sonnet 4.6 rejects (400 `This model does not support
    // effort level 'xhigh'`) — it killed a `deep-research` spawn. Sonnet 5
    // since gained the tier, so the clamp now applies to 4.6 and Haiku only.
    let sonnet = req("claude-sonnet-4-6", Some(ThinkingConfig::enabled(16_000)));
    let normalized = normalize_thinking_for_wire(&sonnet);
    let oc = normalized.output_config.as_ref().expect("effort set");
    assert_eq!(
        oc.effort.anthropic(),
        "high",
        "Sonnet 4.6 must clamp xhigh → high"
    );
    // The serialized JSON proves the wire never carries the rejected tier.
    let body = serde_json::to_value(normalized.as_ref()).unwrap();
    assert_eq!(body["output_config"]["effort"], "high");
    // Gating guard: Sonnet keeps PLAIN adaptive — no `display` field — because
    // its acceptance of `display:"summarized"` is unconfirmed (avoid a 400).
    assert!(
        normalized.thinking.as_ref().unwrap().display.is_none(),
        "Sonnet must not carry display"
    );
    assert!(body["thinking"].get("display").is_none());

    // Opus accepts xhigh and must keep it (no over-clamping of capable models),
    // AND opts into the summarized display.
    let opus = req("claude-opus-4-8", Some(ThinkingConfig::enabled(16_000)));
    let opus_norm = normalize_thinking_for_wire(&opus);
    assert_eq!(
        opus_norm.output_config.as_ref().unwrap().effort.anthropic(),
        "xhigh",
        "Opus keeps xhigh"
    );
    assert_eq!(
        opus_norm.thinking.as_ref().unwrap().display.as_deref(),
        Some("summarized"),
        "Opus opts into summarized display"
    );
}

#[test]
fn adaptive_anthropic_never_serializes_ultra() {
    // Anthropic's scale stops at `max`, so the Zo-internal `Ultra` lands there
    // rather than on a lower rung — and `ultra` itself never reaches the wire.
    for (model, expected) in [("claude-opus-4-8", "max"), ("claude-sonnet-5", "max")] {
        let mut request = req(model, Some(ThinkingConfig::enabled(20_000)));
        request.effort = Some(crate::types::EffortLevel::Ultra);
        let normalized = normalize_thinking_for_wire(&request);
        let body = serde_json::to_value(normalized.as_ref()).unwrap();
        assert_eq!(body["output_config"]["effort"], expected, "{model}");
        assert_ne!(body["output_config"]["effort"], "ultra", "{model}");
    }
}

#[test]
fn prepopulated_anthropic_output_config_never_serializes_ultra() {
    for (model, expected) in [
        ("claude-opus-4-8", "max"),
        ("claude-sonnet-5", "max"),
        ("claude-opus-4-5", "max"),
    ] {
        let mut request = req(model, None);
        request.output_config = Some(OutputConfig::new(crate::types::EffortLevel::Ultra));
        let normalized = normalize_thinking_for_wire(&request);
        let body = serde_json::to_value(normalized.as_ref()).unwrap();
        assert_eq!(body["output_config"]["effort"], expected, "{model}");
        assert_ne!(body["output_config"]["effort"], "ultra", "{model}");
    }
}

#[test]
fn banded_request_escalates_fable_all_the_way_to_max() {
    // Anthropic's `anthropic_for_model` permanently clamps the provider-neutral
    // `Ultra` variant down to `Xhigh` — it never silently upgrades to `Max`.
    // So a Smart-mode band's escalated pick MUST already be a named `Max`
    // (never `Ultra`) by the time it reaches this clamp, or fable could never
    // reach its true `max` ceiling on a heavy turn. Floor stays Xhigh
    // (byte-identical to the pre-band default) when no signal fires.
    let mut trivial = req("claude-fable-5", None);
    trivial.effort = Some(crate::types::EffortLevel::Xhigh);
    trivial.effort_band_ceiling = Some(crate::types::EffortLevel::Ultra);
    let normalized = normalize_thinking_for_wire(&trivial);
    assert_eq!(
        normalized.output_config.as_ref().unwrap().effort.anthropic(),
        "xhigh"
    );

    let mut heavy = req("claude-fable-5", None);
    heavy.messages = vec![InputMessage::user_text("please refactor this module")];
    heavy.effort = Some(crate::types::EffortLevel::Xhigh);
    heavy.effort_band_ceiling = Some(crate::types::EffortLevel::Ultra);
    let normalized = normalize_thinking_for_wire(&heavy);
    let body = serde_json::to_value(normalized.as_ref()).unwrap();
    assert_eq!(
        body["output_config"]["effort"], "max",
        "a heavy-intent Smart-mode turn must reach fable's true ceiling, not clamp to xhigh"
    );
    assert_ne!(body["output_config"]["effort"], "ultra");
}

#[test]
fn adaptive_sonnet_keeps_max_effort_on_wire() {
    // Only `xhigh` is the Sonnet gap — `max` IS in its supported set, so an
    // explicit Max effort must pass through untouched (never down-clamped).
    let mut request = req("claude-sonnet-5", Some(ThinkingConfig::enabled(1_000)));
    request.effort = Some(crate::types::EffortLevel::Max);
    let normalized = normalize_thinking_for_wire(&request);
    assert_eq!(
        normalized
            .output_config
            .as_ref()
            .unwrap()
            .effort
            .anthropic(),
        "max",
        "Sonnet accepts max; only xhigh is clamped"
    );
}

#[test]
fn haiku_keeps_legacy_thinking_and_never_carries_display() {
    // Haiku 4.5 is NOT an adaptive model, so it stays on legacy budget thinking
    // and never reaches the adaptive-rewrite (and hence never the `display`)
    // path — the request passes through untouched with no `display` key.
    let request = req("claude-haiku-4-5", Some(ThinkingConfig::enabled(8_000)));
    let normalized = normalize_thinking_for_wire(&request);
    assert!(matches!(normalized, std::borrow::Cow::Borrowed(_)));
    let thinking = normalized.thinking.as_ref().expect("thinking kept");
    assert_eq!(thinking.kind, "enabled");
    assert!(thinking.display.is_none(), "Haiku must not carry display");
    let body = serde_json::to_value(normalized.as_ref()).unwrap();
    assert!(body["thinking"].get("display").is_none());
}

#[test]
fn explicit_effort_wins_over_budget_on_adaptive_model() {
    let mut request = req("claude-opus-4-8", Some(ThinkingConfig::enabled(1_000)));
    request.effort = Some(crate::types::EffortLevel::Max);
    let normalized = normalize_thinking_for_wire(&request);
    assert_eq!(
        normalized
            .output_config
            .as_ref()
            .unwrap()
            .effort
            .anthropic(),
        "max",
        "explicit effort overrides the budget-derived level"
    );
}

#[test]
fn legacy_model_keeps_budget_thinking_untouched() {
    // Opus 4.5 still takes a real budget; the request must pass through
    // unchanged (borrowed, no clone) with budget_tokens intact.
    let request = req("claude-opus-4-5", Some(ThinkingConfig::enabled(12_000)));
    let normalized = normalize_thinking_for_wire(&request);
    assert!(matches!(normalized, std::borrow::Cow::Borrowed(_)));
    assert_eq!(
        normalized.thinking.as_ref().unwrap().budget_tokens,
        Some(12_000)
    );
    assert!(normalized.output_config.is_none());

    let body = serde_json::to_value(normalized.as_ref()).unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 12_000);
    assert!(body["thinking"].get("display").is_none(), "legacy budget thinking carries no display");
    assert!(body.get("output_config").is_none());
}

#[test]
fn adaptive_model_without_thinking_is_left_untouched() {
    let request = req("claude-opus-4-8", None);
    let normalized = normalize_thinking_for_wire(&request);
    assert!(matches!(normalized, std::borrow::Cow::Borrowed(_)));
    assert!(normalized.thinking.is_none());
    assert!(normalized.output_config.is_none());
}

#[test]
fn adaptive_model_drops_legacy_enabled_shape_even_without_budget() {
    let request = req(
        "claude-opus-4-8",
        Some(ThinkingConfig {
            kind: "enabled".to_string(),
            budget_tokens: None,
            display: None,
            block_binding: None,
        }),
    );
    let normalized = normalize_thinking_for_wire(&request);

    assert!(matches!(normalized, std::borrow::Cow::Owned(_)));
    let thinking = normalized.thinking.as_ref().expect("thinking kept");
    assert_eq!(thinking.kind, "adaptive");
    assert!(thinking.budget_tokens.is_none());
    // The else-branch is gated too: Opus carries the summarized display so the
    // legacy-enabled→adaptive rewrite also streams visible reasoning.
    assert_eq!(thinking.display.as_deref(), Some("summarized"));
    assert!(normalized.output_config.is_none());
}

#[test]
fn output_config_omits_effort_field_when_absent() {
    // OutputConfig isn't emitted at all when there's no effort.
    let _ = OutputConfig::new(crate::types::EffortLevel::Low); // type is constructible
    let request = req("claude-opus-4-5", None);
    let body = serde_json::to_value(&request).unwrap();
    assert!(body.get("output_config").is_none());
}

/// Context editing is **default OFF**: with no env override the builder is a
/// no-op, non-opt-in values stay off, and a latched surface-unsupported flag
/// forces it off regardless of env.
#[test]
fn context_editing_defaults_off_and_latch_forces_off() {
    let _guard = env_lock();
    let _context_edit_guard = ContextEditTestGuard::new();

    // default (unset) → OFF, builder is a no-op
    std::env::remove_var("ZO_ANTHROPIC_CONTEXT_EDIT");
    assert!(!super::anthropic_context_editing_enabled());
    let off = AnthropicClient::from_auth(AuthSource::None).with_env_context_editing();
    assert!(!off
        .request_profile
        .betas
        .iter()
        .any(|beta| beta.starts_with("context-management")));
    assert!(!off.request_profile.extra_body.contains_key("context_management"));

    // only explicit opt-in values enable the gate
    for value in ["", "0", "false", "off", "no", "unexpected"] {
        std::env::set_var("ZO_ANTHROPIC_CONTEXT_EDIT", value);
        assert!(!super::anthropic_context_editing_enabled(), "{value}");
    }

    // latch forces off even with an explicit opt-in
    std::env::set_var("ZO_ANTHROPIC_CONTEXT_EDIT", "1");
    super::CONTEXT_EDIT_SURFACE_UNSUPPORTED.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(!super::anthropic_context_editing_enabled());
}

#[test]
fn context_editing_env_opt_in_attaches_beta_and_edit() {
    let _guard = env_lock();
    let _context_edit_guard = ContextEditTestGuard::new();

    for value in ["1", "true", "on", "yes"] {
        std::env::set_var("ZO_ANTHROPIC_CONTEXT_EDIT", value);
        assert!(super::anthropic_context_editing_enabled(), "{value}");
    }
    let on = AnthropicClient::from_auth(AuthSource::None).with_env_context_editing();
    assert!(on
        .request_profile
        .betas
        .contains(&"context-management-2025-06-27".to_string()));
    assert_eq!(
        on.request_profile.extra_body["context_management"]["edits"][0]["type"],
        "clear_tool_uses_20250919"
    );
    assert_eq!(
        on.request_profile.extra_body["context_management"]["edits"][0]
            ["clear_at_least"],
        serde_json::json!({"type": "input_tokens", "value": 20_000})
    );
}

/// A 400 rejecting the context-management beta is recognized (message OR body),
/// while a non-400 or an unrelated 400 is not — this gates the retry fallback.
#[test]
fn context_edit_unsupported_detection() {
    use crate::error::ApiError;
    let make = |status: reqwest::StatusCode, message: Option<&str>, body: &str| ApiError::Api {
        status,
        error_type: None,
        message: message.map(str::to_string),
        body: body.to_string(),
        retryable: false,
        retry_after: None,
    };
    // 400 naming the beta in the message
    assert!(super::is_context_edit_unsupported(&make(
        reqwest::StatusCode::BAD_REQUEST,
        Some("unsupported anthropic-beta: context-management-2025-06-27"),
        "{}",
    )));
    // 400 naming the edit only in the raw body
    assert!(super::is_context_edit_unsupported(&make(
        reqwest::StatusCode::BAD_REQUEST,
        None,
        "{\"error\":{\"message\":\"clear_tool_uses not allowed\"}}",
    )));
    // right text, wrong status
    assert!(!super::is_context_edit_unsupported(&make(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        Some("context-management"),
        "context_management",
    )));
    // 400 but unrelated
    assert!(!super::is_context_edit_unsupported(&make(
        reqwest::StatusCode::BAD_REQUEST,
        Some("thinking.budget_tokens invalid"),
        "{}",
    )));
}

/// The context-management beta is stripped from the joined `anthropic-beta`
/// header value while the other betas keep their order; absent → unchanged.
#[test]
fn strip_context_management_beta_leaves_others_intact() {
    assert_eq!(
        super::strip_context_management_beta(
            "interleaved-thinking-2025-05-14,context-management-2025-06-27,oauth-2025-04-20"
        ),
        "interleaved-thinking-2025-05-14,oauth-2025-04-20"
    );
    assert_eq!(
        super::strip_context_management_beta("interleaved-thinking-2025-05-14,oauth-2025-04-20"),
        "interleaved-thinking-2025-05-14,oauth-2025-04-20"
    );
}

/// The byte-size preflight rejects an oversized body BEFORE upload, and does so
/// as a real 413 so it lands in the existing `ContextOverflow` recovery instead
/// of a new bespoke error path. An undeclared ceiling must disable the check
/// entirely — "unknown" is not "unlimited", and a guessed limit would reject
/// valid requests.
#[test]
fn request_size_preflight_fires_only_against_a_declared_ceiling() {
    use crate::ProviderErrorClass;

    let declared = crate::providers::max_request_bytes_for_model("claude-opus-5")
        .expect("the catalog declares a request-byte ceiling for Anthropic models");

    let at_limit = usize::try_from(declared).expect("ceiling fits a usize on this target");

    // Under the ceiling: send it.
    assert!(super::request_too_large_preflight("claude-opus-5", at_limit - 1).is_none());
    // Exactly at the ceiling is still acceptable — the limit is inclusive.
    assert!(super::request_too_large_preflight("claude-opus-5", at_limit).is_none());

    // Over the ceiling: a 413 that classifies exactly like the provider's own,
    // so `recover_provider_context_overflow` picks it up.
    let error = super::request_too_large_preflight("claude-opus-5", at_limit + 1)
        .expect("an oversized body must be refused before upload");
    assert_eq!(
        error.provider_error_class(),
        ProviderErrorClass::ContextOverflow
    );
    assert!(!error.is_retryable(), "the identical body would fail again");
    // Both numbers are named: the provider's own 413 carries neither, and
    // "how far over" is what tells a user whether compaction can fix it.
    let rendered = error.to_string();
    assert!(rendered.contains(&declared.to_string()), "{rendered}");
    assert!(rendered.contains(&(declared + 1).to_string()), "{rendered}");

    // An undeclared model must never be preflighted, at any size.
    assert!(crate::providers::max_request_bytes_for_model("some-unlisted-local-model").is_none());
    assert!(
        super::request_too_large_preflight("some-unlisted-local-model", usize::MAX / 2).is_none()
    );
}

/// Fable and Mythos think on every request and refuse to stop, so a request to
/// them with no `thinking` field is still a thinking request: the reasoning
/// blocks in its history are valid and must travel unchanged. Stripping them
/// would edit the history behind them — on Claude Fable 5.1 that invalidates
/// every later thinking block.
#[test]
fn an_always_thinking_model_keeps_its_replayed_thinking_blocks() {
    let mut request = req("claude-fable-5-1", None);
    request.messages.push(InputMessage {
        role: "assistant".to_string(),
        content: vec![
            InputContentBlock::Thinking {
                thinking: String::new(),
                signature: "sig".to_string(),
            },
            InputContentBlock::Text {
                text: "done".to_string(),
                cache_control: None,
            },
        ],
        thought_signature: None,
        reasoning_replay: None,
    });
    let kept = strip_thinking_blocks_when_disabled(std::borrow::Cow::Borrowed(&request));
    assert!(
        matches!(kept, std::borrow::Cow::Borrowed(_)),
        "a Fable request was rewritten"
    );
    assert_eq!(kept.messages[1].content.len(), 2);

    // Opus 4.8 with thinking off is the case the strip exists for.
    let mut legacy = req("claude-opus-4-8", None);
    legacy.messages.clone_from(&request.messages);
    let stripped = strip_thinking_blocks_when_disabled(std::borrow::Cow::Borrowed(&legacy));
    assert_eq!(
        stripped.messages[1].content.len(),
        1,
        "Opus 4.8 without thinking kept a thinking block"
    );
}

/// Claude Fable 5.1 and Mythos 5.1 return 400 on a forced tool choice
/// (`tool_choice: type "tool" and "any" are not supported for this model`).
/// The wire never carries one to them: the request goes out as `auto`, the
/// runtime having already put the requirement in the transcript.
#[test]
fn a_forced_tool_choice_never_reaches_fable_5_1() {
    let mut request = req("claude-fable-5-1", None);
    request.tool_choice = Some(ToolChoice::Tool {
        name: "StructuredOutput".to_string(),
    });
    let wire = normalize_tool_choice_for_wire(std::borrow::Cow::Borrowed(&request));
    assert_eq!(wire.tool_choice, Some(ToolChoice::Auto));

    request.tool_choice = Some(ToolChoice::Any);
    let wire = normalize_tool_choice_for_wire(std::borrow::Cow::Borrowed(&request));
    assert_eq!(wire.tool_choice, Some(ToolChoice::Auto));

    // `none` is unaffected, and so is every other model.
    request.tool_choice = Some(ToolChoice::None);
    let wire = normalize_tool_choice_for_wire(std::borrow::Cow::Borrowed(&request));
    assert!(matches!(wire, std::borrow::Cow::Borrowed(_)));
    let mut opus = req("claude-opus-5", None);
    opus.tool_choice = Some(ToolChoice::Any);
    let wire = normalize_tool_choice_for_wire(std::borrow::Cow::Borrowed(&opus));
    assert_eq!(wire.tool_choice, Some(ToolChoice::Any));
}

/// The 5.1 generation binds each thinking block to the model and the prefix
/// that produced it. A first-party request carries `block_binding: drop_block`
/// (the beta header travels with it) so a prefix this harness changed degrades
/// to a re-plan instead of a 400; a gateway request does not, because Bedrock
/// and Vertex reject the header until they carry the control.
#[test]
fn fable_5_1_binds_its_thinking_on_first_party_requests_only() {
    let request = req("claude-fable-5-1", None);
    let bound = bind_thinking_for_wire(std::borrow::Cow::Borrowed(&request), true);
    let thinking = bound.thinking.as_ref().expect("thinking attached");
    assert_eq!(thinking.kind, "adaptive");
    assert_eq!(thinking.block_binding, Some(BlockBinding::drop_block()));
    assert!(matches!(
        bind_thinking_for_wire(std::borrow::Cow::Borrowed(&request), false),
        std::borrow::Cow::Borrowed(_)
    ));
    // Fable 5 has no such control.
    let fable_5 = req("claude-fable-5", None);
    assert!(matches!(
        bind_thinking_for_wire(std::borrow::Cow::Borrowed(&fable_5), true),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn the_binding_beta_joins_the_header_once() {
    assert_eq!(
        with_beta("a-1,b-2", THINKING_BINDING_BETA),
        format!("a-1,b-2,{THINKING_BINDING_BETA}")
    );
    assert_eq!(
        with_beta(&format!("a-1,{THINKING_BINDING_BETA}"), THINKING_BINDING_BETA),
        format!("a-1,{THINKING_BINDING_BETA}")
    );
    assert_eq!(with_beta("", THINKING_BINDING_BETA), THINKING_BINDING_BETA);
}

/// A `retry-after` the ladder could never reach — hours, when the ladder's
/// whole reach is minutes — ends the ladder after the first answer, with the
/// hint kept on the error for the runtime's escape. Measured on 2026-09-21:
/// each request slept 30 s up to six times against a 154,912 s hint.
#[tokio::test]
async fn a_wall_hours_away_is_not_retried_at_the_ladders_cap() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let server_hits = hits.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_millis(300), listener.accept()).await
            else {
                break;
            };
            server_hits.fetch_add(1, Ordering::SeqCst);
            let mut scratch = [0u8; 2048];
            let _ = socket.read(&mut scratch).await;
            let body = b"{\"error\":{\"type\":\"rate_limit_error\",\"message\":\"This request would exceed your account's rate limit.\"},\"type\":\"error\"}";
            let head = format!(
                "HTTP/1.1 429 Too Many Requests\r\nconnection: close\r\nretry-after: 154912\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
            socket.flush().await.unwrap();
            let _ = socket.shutdown().await;
        }
    });

    let client = AnthropicClient::new("token")
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(5, Duration::from_millis(1), Duration::from_millis(5));
    let began = std::time::Instant::now();
    let error = client
        .stream_message(&streaming_request())
        .await
        .expect_err("a wall hours away is an error, not a wait");
    let took = began.elapsed();
    server.await.unwrap();

    assert_eq!(hits.load(Ordering::SeqCst), 1, "the ladder asked again against a hint it cannot reach");
    assert!(took < Duration::from_secs(5), "the ladder slept against the wall: {took:?}");
    assert_eq!(
        error.retry_after(),
        Some(Duration::from_secs(154_912)),
        "the hint must ride up with the error: {error}"
    );
}
