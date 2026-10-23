use std::borrow::Cow;
use std::fmt::Write as _;
use std::io::Read;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::{Host, Url};

mod openai;
mod protocol;
pub use protocol::Prepared;

pub const SETTINGS_KEY: &str = "judgmentProvider";
pub const DEADLINE_HEADER: &str = "x-zerocode-judgment-deadline-ms";
pub const JEV_BASE_URL: &str = "https://api.typesafe.ai";
pub const JEV_DEFAULT_MODEL: &str = "jev-latest";
pub const JEV_KEY_ENV: &str = "TYPESAFE_API_KEY";
pub const CUSTOM_KEY_ENV_PREFIX: &str = "JUDGMENT_API_KEY_";
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ADDRESS_BYTES: usize = 8 * 1024;
pub const MAX_MODELS: usize = 2_000;
const ROUTE_FINGERPRINT_BYTES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    Jev,
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    #[default]
    SystemOne,
    OpenaiJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provider {
    pub kind: ProviderKind,
    pub protocol: Protocol,
    pub base_url: String,
    pub model: String,
    pub requires_auth: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Settings,
    Address,
    LocalAddress,
    RemoteAddress,
    Model,
    Request,
    Response,
    TooLarge,
    SettingsChanged,
    Truncated,
    DeadlineBudget,
}

impl Refusal {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Settings => "provider_settings",
            Self::Address => "provider_address",
            Self::LocalAddress => "provider_not_loopback",
            Self::RemoteAddress => "provider_https_required",
            Self::Model => "provider_model",
            Self::Request => "provider_request",
            Self::Response => "schema",
            Self::TooLarge => "provider_response_too_large",
            Self::SettingsChanged => "settings_changed",
            Self::Truncated => "provider_context_truncated",
            Self::DeadlineBudget => "provider_deadline_budget",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.token())
    }
}

impl std::error::Error for Refusal {}

impl Provider {
    pub fn read(path: &Path) -> Result<Self, Refusal> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::from_root(&Value::Null);
            }
            Err(_) => return Err(Refusal::Settings),
        };
        let mut bytes = Vec::new();
        file.take(MAX_RESPONSE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Refusal::Settings)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(Refusal::Settings);
        }
        let root: Value = serde_json::from_slice(&bytes).map_err(|_| Refusal::Settings)?;
        if !root.is_object() {
            return Err(Refusal::Settings);
        }
        Self::from_root(&root)
    }

    #[must_use]
    pub fn evidence_dir(&self, base: &Path) -> PathBuf {
        if self.kind == ProviderKind::Jev {
            base.to_path_buf()
        } else {
            base.join("providers").join(self.identity())
        }
    }

    #[must_use]
    pub fn jev(model: &str) -> Self {
        Self {
            kind: ProviderKind::Jev,
            protocol: Protocol::SystemOne,
            base_url: JEV_BASE_URL.to_string(),
            model: model.to_string(),
            requires_auth: true,
        }
    }

    pub fn from_root(root: &Value) -> Result<Self, Refusal> {
        let smart = root.get("smart");
        let legacy_model = smart
            .and_then(|smart| smart.get("jevModel"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| {
                !model.is_empty()
                    && !model
                        .chars()
                        .any(|letter| letter.is_whitespace() || letter.is_control())
            })
            .unwrap_or(JEV_DEFAULT_MODEL);
        let Some(config) = smart.and_then(|smart| smart.get(SETTINGS_KEY)) else {
            return Ok(Self::jev(legacy_model));
        };
        let provider: Self =
            serde_json::from_value(config.clone()).map_err(|_| Refusal::Settings)?;
        let provider = provider.validated()?;
        if provider.kind == ProviderKind::Jev {
            return Ok(Self::jev(legacy_model));
        }
        Ok(provider)
    }

    pub fn validated(self) -> Result<Self, Refusal> {
        let provider = self.endpoint_validated()?;
        if !valid_model(&provider.model) {
            return Err(Refusal::Model);
        }
        Ok(provider)
    }

    pub fn endpoint_validated(mut self) -> Result<Self, Refusal> {
        if self.kind == ProviderKind::Jev
            && (self.protocol != Protocol::SystemOne
                || self.base_url != JEV_BASE_URL
                || !self.requires_auth)
        {
            return Err(Refusal::Settings);
        }
        self.model = self.model.trim().to_string();
        if self.base_url.len() > MAX_ADDRESS_BYTES
            || self.base_url.trim().chars().any(char::is_control)
        {
            return Err(Refusal::Address);
        }
        let address = Url::parse(self.base_url.trim()).map_err(|_| Refusal::Address)?;
        if address.host().is_none()
            || !matches!(address.scheme(), "http" | "https")
            || !address.username().is_empty()
            || address.password().is_some()
            || address.query().is_some()
            || address.fragment().is_some()
        {
            return Err(Refusal::Address);
        }
        match self.kind {
            ProviderKind::Local if !loopback(&address) => return Err(Refusal::LocalAddress),
            ProviderKind::Remote if address.scheme() != "https" && !private_network(&address) => {
                return Err(Refusal::RemoteAddress);
            }
            _ => {}
        }
        self.base_url = address.as_str().trim_end_matches('/').to_string();
        Ok(self)
    }

    #[must_use]
    pub fn key_env(&self) -> Cow<'static, str> {
        match self.kind {
            ProviderKind::Jev => Cow::Borrowed(JEV_KEY_ENV),
            ProviderKind::Local | ProviderKind::Remote => {
                let mut hash = Sha256::new();
                hash.update(self.base_url.as_bytes());
                hash.update([0]);
                hash.update(format!("{:?}", self.protocol).as_bytes());
                Cow::Owned(format!("{CUSTOM_KEY_ENV_PREFIX}{:X}", hash.finalize()))
            }
        }
    }

    #[must_use]
    pub fn ready_with_key(&self, key: Option<&str>) -> bool {
        !self.requires_auth || key.is_some_and(|key| !key.trim().is_empty())
    }

    #[must_use]
    pub fn api_url(&self, resource: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let version = if base.ends_with("/v1") { "" } else { "/v1" };
        format!("{base}{version}/{resource}")
    }

    #[must_use]
    pub fn request_url(&self) -> String {
        self.api_url(match self.protocol {
            Protocol::SystemOne => "systemone",
            Protocol::OpenaiJson => "chat/completions",
        })
    }

    #[must_use]
    pub fn identity(&self) -> String {
        if self.kind == ProviderKind::Jev {
            return self.model.clone();
        }
        let mut hash = Sha256::new();
        for part in [
            format!("{:?}", self.kind),
            format!("{:?}", self.protocol),
            self.base_url.clone(),
            self.model.clone(),
            self.requires_auth.to_string(),
        ] {
            hash.update(part.as_bytes());
            hash.update([0]);
        }
        format!("judgment-{:x}", hash.finalize())
    }

    #[must_use]
    pub fn route_fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        for part in [
            "route".to_string(),
            format!("{:?}", self.kind),
            format!("{:?}", self.protocol),
            self.base_url.clone(),
            self.model.clone(),
            self.requires_auth.to_string(),
        ] {
            hash.update(part.as_bytes());
            hash.update([0]);
        }
        let digest = hash.finalize();
        let mut route = String::from("route-");
        for byte in &digest[..ROUTE_FINGERPRINT_BYTES] {
            let _ = write!(route, "{byte:02x}");
        }
        route
    }
}

fn loopback(address: &Url) -> bool {
    match address.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn valid_model(model: &str) -> bool {
    !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control)
}

/// Explicit LAN endpoints can use HTTP, as local model servers do. Domain
/// names still require TLS: validation never guesses where DNS will send a
/// later request. Link-local and public addresses are not LAN exceptions.
fn private_network(address: &Url) -> bool {
    if loopback(address) {
        return true;
    }
    match address.host() {
        Some(Host::Ipv4(address)) => address.is_private(),
        Some(Host::Ipv6(address)) => address.is_unique_local(),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
}

/// What a local server declared about the model it serves: the token limits
/// of the process that loaded it, that process's runtime build, and the
/// server start that loaded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub model: String,
    pub runtime: String,
    pub generation: String,
    pub tokens: TokenLimits,
}

/// Counts in the loaded model's own tokens: `max_len`, the longest input it
/// takes, and `head_max_len`, each question's head budget, when its runtime
/// has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TokenLimits {
    pub max_len: NonZeroU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_max_len: Option<NonZeroU64>,
}

impl TokenLimits {
    /// The counts `tokens` declares, or `None` unless its `max_len`, and its
    /// `head_max_len` when it has one, are positive integers.
    #[must_use]
    pub fn read(tokens: &Value) -> Option<Self> {
        let count = |count: &Value| count.as_u64().and_then(NonZeroU64::new);
        let head_max_len = match tokens.get("head_max_len") {
            Some(head_max_len) => Some(count(head_max_len)?),
            None => None,
        };
        Some(Self {
            max_len: count(tokens.get("max_len")?)?,
            head_max_len,
        })
    }
}

impl Limits {
    /// The limits `limits` declares for model `id`, or `None` unless they name
    /// that model, a runtime build, a server start and usable token counts.
    fn read(limits: &Value, id: &str) -> Option<Self> {
        let name = |key: &str| {
            limits
                .get(key)
                .and_then(Value::as_str)
                .filter(|name| valid_model(name))
                .map(str::to_owned)
        };
        Some(Self {
            model: name("model").filter(|model| model == id)?,
            runtime: name("runtime")?,
            generation: name("generation")?,
            tokens: TokenLimits::read(limits.get("tokens")?)?,
        })
    }
}

pub fn read_models(body: &[u8]) -> Result<Vec<Model>, Refusal> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(Refusal::TooLarge);
    }
    let document: Value = serde_json::from_slice(body).map_err(|_| Refusal::Response)?;
    let entries = document
        .get("data")
        .or_else(|| document.get("models"))
        .and_then(Value::as_array)
        .ok_or(Refusal::Response)?;
    if entries.len() > MAX_MODELS {
        return Err(Refusal::TooLarge);
    }
    let mut models = std::collections::BTreeMap::new();
    for entry in entries {
        let name = entry
            .as_str()
            .or_else(|| entry.get("id").and_then(Value::as_str))
            .or_else(|| entry.get("name").and_then(Value::as_str))
            .ok_or(Refusal::Response)?;
        if !valid_model(name) {
            return Err(Refusal::Model);
        }
        let limits = entry
            .get("limits")
            .and_then(|limits| Limits::read(limits, name));
        match models.entry(name.to_string()) {
            std::collections::btree_map::Entry::Vacant(model) => {
                model.insert(limits);
            }
            std::collections::btree_map::Entry::Occupied(mut model) => {
                if *model.get() != limits {
                    model.insert(None);
                }
            }
        }
    }
    Ok(models
        .into_iter()
        .map(|(id, limits)| Model { id, limits })
        .collect())
}

#[cfg(test)]
mod tests;
