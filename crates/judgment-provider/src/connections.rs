//! Shared interpretation of model connection settings.
//!
//! Credential requirements and transport permission are different decisions.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{Protocol, Provider, ProviderKind, Refusal};

pub const LAUNCH_ROUTES_ENV: &str = "ZEROCODE_CONNECTION_ROUTES";
pub const LAUNCH_ROUTES_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRoutes {
    pub version: u32,
    pub routes: BTreeMap<String, Vec<String>>,
}

pub const ROUTER_KEY_ENV_PREFIX: &str = "ZEROCODE_ROUTER_";
pub const RELOAD_METHOD: &str = "providers.reload";
pub const RELOAD_VERSION: u64 = 1;

pub const JUDGMENT_CONNECTION_KEY: &str = "judgmentConnection";
pub const CODING_CONNECTION_KEY: &str = "codingConnection";
pub const PREVIOUS_CODING_KEY: &str = "previousCodingSelection";
pub const PREVIOUS_JUDGMENT_KEY: &str = "previousJudgmentSelection";

/// A role names a saved connection, never an address paired with an arbitrary key reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reference {
    pub id: String,
    pub generation: u64,
    pub model: String,
    pub protocol: Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authentication {
    pub header: String,
    pub scheme: Option<String>,
}

impl Default for Authentication {
    fn default() -> Self {
        Self {
            header: "Authorization".to_string(),
            scheme: Some("Bearer".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub id: String,
    pub generation: u64,
    #[serde(default)]
    pub auth: Authentication,
}

/// Request-only information, excluded from the five-field legacy provider document.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Binding {
    pub reference: Reference,
    pub auth_env: String,
    pub auth: Authentication,
    pub headers: BTreeMap<String, String>,
    pub client_fingerprint: Option<String>,
    pub user_agent: Option<String>,
}

impl std::fmt::Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Binding")
            .field("reference", &self.reference)
            .finish_non_exhaustive()
    }
}

/// Legacy rows receive a deterministic identity without changing their document.
/// Saving or explicitly selecting the row persists that same identity.
pub fn metadata(row: &Value) -> Result<Metadata, Refusal> {
    if let Some(value) = row.get("connection") {
        let metadata: Metadata =
            serde_json::from_value(value.clone()).map_err(|_| Refusal::Settings)?;
        if metadata.id.is_empty()
            || metadata.id.len() > 128
            || !metadata
                .id
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_')
        {
            return Err(Refusal::Settings);
        }
        return Ok(metadata);
    }
    let mut hash = Sha256::new();
    for key in ["name", "auth_env"] {
        hash.update(
            row.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .as_bytes(),
        );
        hash.update([0]);
    }
    Ok(Metadata {
        aliases: Vec::new(),
        id: format!("connection-{:x}", hash.finalize()),
        generation: 0,
        auth: Authentication::default(),
    })
}

pub fn row<'a>(root: &'a Value, id: &str) -> Result<&'a Value, Refusal> {
    let mut matched = root
        .get("providers")
        .and_then(Value::as_array)
        .ok_or(Refusal::SettingsChanged)?
        .iter()
        .filter(|row| metadata(row).is_ok_and(|metadata| metadata.id == id));
    let row = matched.next().ok_or(Refusal::SettingsChanged)?;
    if matched.next().is_some() {
        return Err(Refusal::Settings);
    }
    Ok(row)
}

#[must_use]
pub fn requires_auth(row: &Value) -> bool {
    row.get("requires_auth")
        .and_then(Value::as_bool)
        .unwrap_or(true)
        || (row
            .get("auth_env")
            .and_then(Value::as_str)
            .is_some_and(|name| name.starts_with(ROUTER_KEY_ENV_PREFIX))
            && !router_base_is_local(
                row.get("base_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ))
}

/// Non-secret route stamp; display names and model-list refreshes do not alter it.
pub fn route_stamp(row: &Value) -> Result<String, Refusal> {
    let metadata = metadata(row)?;
    let mut hash = Sha256::new();
    for key in [
        "base_url",
        "auth_env",
        "client_fingerprint",
        "headers",
        "user_agent",
    ] {
        let value = row.get(key).filter(|value| !value.is_null());
        let rendered = if key == "headers" {
            value
                .map(|value| {
                    let headers: BTreeMap<String, String> =
                        serde_json::from_value(value.clone()).map_err(|_| Refusal::Settings)?;
                    serde_json::to_string(&headers).map_err(|_| Refusal::Settings)
                })
                .transpose()?
                .unwrap_or_default()
        } else {
            value.map(Value::to_string).unwrap_or_default()
        };
        hash.update(rendered.as_bytes());
        hash.update([0]);
    }
    hash.update(requires_auth(row).to_string().as_bytes());
    hash.update(metadata.id.as_bytes());
    hash.update(metadata.generation.to_be_bytes());
    hash.update(metadata.auth.header.as_bytes());
    hash.update([0]);
    hash.update(
        metadata
            .auth
            .scheme
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
    );
    Ok(format!("connection-route-{:x}", hash.finalize()))
}

pub fn candidate(root: &Value, reference: &Reference) -> Result<Provider, Refusal> {
    let row = row(root, &reference.id)?;
    let metadata = metadata(row)?;
    if metadata.generation != reference.generation {
        return Err(Refusal::SettingsChanged);
    }
    let base_url = row
        .get("base_url")
        .and_then(Value::as_str)
        .ok_or(Refusal::Address)?;
    let address = url::Url::parse(base_url).map_err(|_| Refusal::Address)?;
    let auth_env = row
        .get("auth_env")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let requires_auth = requires_auth(row);
    if requires_auth
        && (auth_env.is_empty()
            || !auth_env
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_'))
    {
        return Err(Refusal::Settings);
    }
    let headers = row
        .get("headers")
        .filter(|value| !value.is_null())
        .map(|value| serde_json::from_value(value.clone()).map_err(|_| Refusal::Settings))
        .transpose()?
        .unwrap_or_default();
    Provider {
        kind: if crate::loopback(&address) {
            ProviderKind::Local
        } else {
            ProviderKind::Remote
        },
        protocol: reference.protocol,
        base_url: base_url.to_string(),
        model: reference.model.clone(),
        requires_auth,
        connection: Some(Binding {
            reference: reference.clone(),
            auth_env: auth_env.to_string(),
            auth: metadata.auth,
            headers,
            user_agent: row
                .get("user_agent")
                .and_then(Value::as_str)
                .map(str::to_string),
            client_fingerprint: row
                .get("client_fingerprint")
                .and_then(Value::as_str)
                .map(str::to_string),
        }),
    }
    .validated()
}

pub(crate) fn bind_selected(root: &Value, provider: &mut Provider) -> Result<(), Refusal> {
    let Some(value) = root
        .get("smart")
        .and_then(|smart| smart.get(JUDGMENT_CONNECTION_KEY))
    else {
        return Ok(());
    };
    let reference: Reference =
        serde_json::from_value(value.clone()).map_err(|_| Refusal::Settings)?;
    let candidate = candidate(root, &reference)?;
    if candidate.kind != provider.kind
        || candidate.protocol != provider.protocol
        || candidate.base_url != provider.base_url
        || candidate.model != provider.model
        || candidate.requires_auth != provider.requires_auth
    {
        return Err(Refusal::SettingsChanged);
    }
    provider.connection = candidate.connection;
    Ok(())
}

/// The CLI's existing name equivalence, also used to reject ambiguous new names.
#[must_use]
pub fn names_match(left: &str, right: &str) -> bool {
    let left = left.trim();
    let right = right.trim();
    left.eq_ignore_ascii_case(right)
        || left.bytes().any(|ch| ch.is_ascii_alphanumeric())
            && left
                .bytes()
                .filter(u8::is_ascii_alphanumeric)
                .map(|ch| ch.to_ascii_lowercase())
                .eq(right
                    .bytes()
                    .filter(u8::is_ascii_alphanumeric)
                    .map(|ch| ch.to_ascii_lowercase()))
}

/// The CLI's locality rule for optional authentication on window-owned routers.
/// This does not authorize HTTP transport or resolve a hostname through DNS.
#[must_use]
pub fn router_base_is_local(base_url: &str) -> bool {
    let Some(host) = url::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return ip_is_local(ip);
    }
    let name = host.trim_end_matches('.');
    !name.contains('.')
        || [".localhost", ".local", ".internal", ".home.arpa", ".lan"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn ip_is_local(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                // 100.64.0.0/10, the shared space a carrier NAT or a tailnet hands out.
                || (first == 100 && (second & 0xc0) == 64)
        }
        std::net::IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            ip.to_ipv4_mapped()
                .is_some_and(|mapped| ip_is_local(mapped.into()))
                || ip.is_loopback()
                || ip.is_unspecified()
                // fc00::/7 unique local, fe80::/10 link-local.
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
    }
}
