//! OAuth credential helpers for the runtime.
//!
//! The filesystem persistence and PKCE primitives are the single source of
//! truth in [`api::oauth_store`] (the `runtime` crate depends on `api`). They
//! are re-exported here so existing `runtime::oauth::*` consumers compile
//! unchanged. The only logic that lives in this module is the per-server MCP
//! token storage, which layers a nested `mcp_oauth` map on top of the shared
//! credentials file and is needed only by the runtime.

use std::io;

use serde_json::{Map, Value};

pub use api::oauth_store::{
    clear_oauth_credentials, clear_openai_oauth, code_challenge_s256, credentials_path,
    generate_pkce_pair, generate_state, load_oauth_credentials, load_openai_oauth,
    loopback_redirect_uri, save_oauth_credentials, save_openai_oauth,
};
pub use core_types::oauth::{parse_oauth_callback_query, parse_oauth_callback_request_target};
// Re-export OAuth types from core-types so that existing consumers of
// `runtime::OAuthTokenSet`, etc. continue to compile unchanged.
pub use core_types::{
    OAuthAuthorizationRequest, OAuthCallbackParams, OAuthRefreshRequest, OAuthTokenExchangeRequest,
    OAuthTokenSet, OpenAiOAuthTokens, PkceChallengeMethod, PkceCodePair,
};

// --- Per-server MCP OAuth token storage ---
//
// MCP tokens are keyed by server name under a nested `mcp_oauth` object in the
// shared credentials file, built on the public `api::oauth_store` primitives.

const MCP_OAUTH_KEY: &str = "mcp_oauth";

/// Load OAuth tokens for a specific MCP server.
pub fn load_mcp_oauth_token(server_name: &str) -> io::Result<Option<OAuthTokenSet>> {
    let root = api::oauth_store::read_credentials_root(&credentials_path()?)?;
    let Some(entry) = root
        .get(MCP_OAUTH_KEY)
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(server_name))
    else {
        return Ok(None);
    };
    if entry.is_null() {
        return Ok(None);
    }
    api::oauth_store::token_set_from_value(entry).map(Some)
}

/// Save OAuth tokens for a specific MCP server.
pub fn save_mcp_oauth_token(server_name: &str, token_set: &OAuthTokenSet) -> io::Result<()> {
    api::oauth_store::update_credentials_root(&credentials_path()?, |root| {
        let mcp_oauth = root
            .entry(MCP_OAUTH_KEY)
            .or_insert_with(|| Value::Object(Map::new()));
        let servers = mcp_oauth.as_object_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "mcp_oauth must be an object")
        })?;
        servers.insert(
            server_name.to_owned(),
            api::oauth_store::token_set_to_value(token_set)?,
        );
        Ok(())
    })
}

/// Remove OAuth tokens for a specific MCP server.
pub fn clear_mcp_oauth_token(server_name: &str) -> io::Result<()> {
    api::oauth_store::update_credentials_root(&credentials_path()?, |root| {
        if let Some(servers) = root.get_mut(MCP_OAUTH_KEY).and_then(Value::as_object_mut) {
            servers.remove(server_name);
        }
        Ok(())
    })
}

/// List all MCP servers with stored OAuth tokens.
pub fn list_mcp_oauth_servers() -> io::Result<Vec<String>> {
    let root = api::oauth_store::read_credentials_root(&credentials_path()?)?;
    let Some(servers) = root.get(MCP_OAUTH_KEY).and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    Ok(servers.keys().cloned().collect())
}

/// Check if a stored MCP token is expired (with 60-second buffer).
#[must_use]
pub fn is_mcp_token_expired(token: &OAuthTokenSet) -> bool {
    let Some(expires_at) = token.expires_at else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now + 60 >= expires_at
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        is_mcp_token_expired, list_mcp_oauth_servers, load_mcp_oauth_token, save_mcp_oauth_token,
        OAuthTokenSet,
    };

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn temp_config_home() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "runtime-oauth-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    #[test]
    fn mcp_oauth_per_server_round_trip() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ZO_CONFIG_HOME", &config_home);
        let path = super::credentials_path().expect("path");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");

        let token_a = OAuthTokenSet {
            access_token: "tok-a".to_string(),
            refresh_token: Some("ref-a".to_string()),
            expires_at: Some(9_999_999_999),
            scopes: vec!["read".to_string()],
        };
        let token_b = OAuthTokenSet {
            access_token: "tok-b".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
        };

        save_mcp_oauth_token("server-a", &token_a).expect("save a");
        save_mcp_oauth_token("server-b", &token_b).expect("save b");

        assert_eq!(
            load_mcp_oauth_token("server-a").expect("load a"),
            Some(token_a)
        );
        assert_eq!(
            load_mcp_oauth_token("server-b").expect("load b"),
            Some(token_b)
        );
        assert_eq!(load_mcp_oauth_token("server-c").expect("load c"), None);

        let servers = list_mcp_oauth_servers().expect("list");
        assert!(servers.contains(&"server-a".to_string()));
        assert!(servers.contains(&"server-b".to_string()));

        super::clear_mcp_oauth_token("server-a").expect("clear a");
        assert_eq!(
            load_mcp_oauth_token("server-a").expect("load cleared"),
            None
        );
        assert!(load_mcp_oauth_token("server-b")
            .expect("load b after clear a")
            .is_some());

        std::env::remove_var("ZO_CONFIG_HOME");
        std::fs::remove_dir_all(config_home).expect("cleanup");
    }

    #[test]
    fn is_mcp_token_expired_checks_buffer() {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let fresh = OAuthTokenSet {
            access_token: "t".to_string(),
            refresh_token: None,
            expires_at: Some(now_secs + 300),
            scopes: vec![],
        };
        assert!(!is_mcp_token_expired(&fresh));

        let expiring_soon = OAuthTokenSet {
            expires_at: Some(now_secs + 30), // within 60s buffer
            ..fresh.clone()
        };
        assert!(is_mcp_token_expired(&expiring_soon));

        let no_expiry = OAuthTokenSet {
            expires_at: None,
            ..fresh
        };
        assert!(!is_mcp_token_expired(&no_expiry));
    }

    #[test]
    fn lower_root_mcp_token_clear_stays_cleared() {
        let _guard = env_lock();
        let primary = temp_config_home();
        let lower = temp_config_home();
        let home = temp_config_home();
        let prior: Vec<(&str, Option<std::ffi::OsString>)> = ["ZO_CONFIG_HOME", "ZO_HOME", "HOME"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("ZO_CONFIG_HOME", &primary);
        std::env::set_var("ZO_HOME", &lower);
        std::env::set_var("HOME", &home);

        std::fs::create_dir_all(&lower).expect("lower dir");
        std::fs::write(
            lower.join("credentials.json"),
            r#"{"mcp_oauth":{"server-a":{"accessToken":"tok-a","refreshToken":"ref-a","expiresAt":9999999999,"scopes":["read"]},"server-b":{"accessToken":"tok-b","refreshToken":null,"expiresAt":null,"scopes":[]}}}
"#,
        )
        .expect("seed lower mcp tokens");

        // Both lower-root servers are visible through the merged view.
        assert!(load_mcp_oauth_token("server-a").expect("load a").is_some());
        assert!(load_mcp_oauth_token("server-b").expect("load b").is_some());
        let servers = list_mcp_oauth_servers().expect("list");
        assert!(servers.contains(&"server-a".to_string()));
        assert!(servers.contains(&"server-b".to_string()));

        // Primary-only logout of one server.
        super::clear_mcp_oauth_token("server-a").expect("clear a");

        // A fresh read re-merges the untouched lower root; server-a stays gone
        // while the other per-server entry survives.
        assert!(load_mcp_oauth_token("server-a").expect("reload a").is_none());
        assert!(load_mcp_oauth_token("server-b").expect("reload b").is_some());
        assert!(!list_mcp_oauth_servers()
            .expect("relist")
            .contains(&"server-a".to_string()));

        // The primary file records a per-entry `null` tombstone for server-a.
        let primary_json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(primary.join("credentials.json")).expect("primary creds"),
        )
        .expect("primary json");
        assert_eq!(
            primary_json.get("mcp_oauth").and_then(|v| v.get("server-a")),
            Some(&serde_json::Value::Null)
        );

        for (key, value) in prior {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&primary);
        let _ = std::fs::remove_dir_all(&lower);
        let _ = std::fs::remove_dir_all(&home);
    }
}
