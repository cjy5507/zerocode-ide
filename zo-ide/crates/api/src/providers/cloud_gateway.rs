//! Cloud-gateway routing for Claude: Amazon Bedrock and Google Vertex AI
//! (Claude Code `CLAUDE_CODE_USE_BEDROCK` / `CLAUDE_CODE_USE_VERTEX` parity).
//!
//! Both gateways serve the standard Messages API with SSE streaming, so the
//! whole client (request shape, stream parser, retry/restart machinery) is
//! reused; only three things change, all applied at the single
//! `send_raw_request` chokepoint every Anthropic HTTP request flows through:
//!
//! - **Bedrock** (`https://bedrock-mantle.{region}.api.aws/anthropic`): model
//!   ids carry an `anthropic.` prefix without date suffixes. Auth is either
//!   the `AWS_BEARER_TOKEN_BEDROCK` token sent as `x-api-key` (no signing), or
//!   `SigV4` request signing from resolved AWS credentials (env keys or the
//!   `~/.aws/credentials` profile) — see [`super::aws_sigv4`].
//! - **Vertex** (`https://{region}-aiplatform.googleapis.com`): the model
//!   moves out of the body into the URL
//!   (`…/publishers/anthropic/models/{model}:streamRawPredict`),
//!   `anthropic_version: "vertex-2023-10-16"` moves into the body, and auth
//!   is a Google access token resolved in order: `ANTHROPIC_VERTEX_ACCESS_TOKEN`,
//!   then Application Default Credentials (service-account JWT or
//!   authorized-user refresh, see [`super::google_auth`]), then
//!   `gcloud auth print-access-token`. Cached 45 minutes.
//!
//! Resolution happens once per process (the env contract is process-stable),
//! so foreground turns, sub-agents, and mid-stream restarts all route
//! identically with zero construction-site changes.

use std::sync::OnceLock;
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::error::ApiError;
/// Body version Vertex expects in place of the `anthropic-version` header.
const VERTEX_ANTHROPIC_VERSION: &str = "vertex-2023-10-16";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BedrockAuth {
    /// `AWS_BEARER_TOKEN_BEDROCK` sent as `x-api-key` (no signing).
    Bearer(String),
    /// `SigV4` request signing from resolved AWS credentials. `region` is held
    /// here too because the signature scope binds it.
    SigV4 {
        credentials: Box<super::aws_sigv4::AwsCredentials>,
        region: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloudGateway {
    Bedrock {
        base_url: String,
        auth: BedrockAuth,
    },
    Vertex {
        base_url: String,
        project: String,
        region: String,
    },
}

/// The process-wide gateway decision: `None` (first-party API), or the
/// resolved gateway, or the configuration error to surface on every request.
static ACTIVE: OnceLock<Option<Result<CloudGateway, String>>> = OnceLock::new();

pub(crate) fn active() -> Option<&'static Result<CloudGateway, String>> {
    ACTIVE
        .get_or_init(|| resolve_from_lookup(&|key| std::env::var(key).ok()))
        .as_ref()
}

/// Whether requests are routed through a cloud gateway (used by callers that
/// must not attach first-party-only request decorations, e.g. the OAuth beta
/// header).
#[must_use]
pub fn cloud_gateway_active() -> bool {
    active().is_some()
}

fn flag(lookup: &dyn Fn(&str) -> Option<String>, key: &str) -> bool {
    lookup(key).is_some_and(|v| {
        let v = v.trim();
        !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    })
}

fn resolve_from_lookup(
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<Result<CloudGateway, String>> {
    let bedrock = flag(lookup, "ZO_USE_BEDROCK") || flag(lookup, "CLAUDE_CODE_USE_BEDROCK");
    let vertex = flag(lookup, "ZO_USE_VERTEX") || flag(lookup, "CLAUDE_CODE_USE_VERTEX");
    match (bedrock, vertex) {
        (false, false) => None,
        (true, true) => Some(Err(
            "both Bedrock and Vertex gateways are enabled; set only one of \
             ZO_USE_BEDROCK / ZO_USE_VERTEX"
                .to_string(),
        )),
        (true, false) => Some(resolve_bedrock(lookup)),
        (false, true) => Some(resolve_vertex(lookup)),
    }
}

fn resolve_bedrock(lookup: &dyn Fn(&str) -> Option<String>) -> Result<CloudGateway, String> {
    let region = lookup("AWS_REGION")
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "us-east-1".to_string());
    let base_url = lookup("ANTHROPIC_BEDROCK_BASE_URL")
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| format!("https://bedrock-mantle.{region}.api.aws/anthropic"));
    // Bearer token wins when present (cheapest, no signing); otherwise sign
    // with resolved AWS credentials (env or shared credentials file).
    let auth = if let Some(token) = lookup("AWS_BEARER_TOKEN_BEDROCK")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        BedrockAuth::Bearer(token)
    } else if let Some(credentials) = super::aws_sigv4::resolve_credentials(lookup) {
        BedrockAuth::SigV4 {
            credentials: Box::new(credentials),
            region: region.clone(),
        }
    } else {
        return Err(
            "Bedrock gateway is enabled but no credentials were found. Set \
             AWS_BEARER_TOKEN_BEDROCK, or AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY, \
             or configure ~/.aws/credentials (AWS_PROFILE selects the profile)."
                .to_string(),
        );
    };
    Ok(CloudGateway::Bedrock { base_url, auth })
}

fn resolve_vertex(lookup: &dyn Fn(&str) -> Option<String>) -> Result<CloudGateway, String> {
    let project = lookup("ANTHROPIC_VERTEX_PROJECT_ID")
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| {
            "Vertex gateway is enabled but ANTHROPIC_VERTEX_PROJECT_ID is not set".to_string()
        })?;
    let region = lookup("CLOUD_ML_REGION")
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "global".to_string());
    let base_url = lookup("ANTHROPIC_VERTEX_BASE_URL")
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| format!("https://{region}-aiplatform.googleapis.com"));
    Ok(CloudGateway::Vertex {
        base_url,
        project,
        region,
    })
}

impl CloudGateway {
    /// Request URL for this gateway. Vertex encodes the model and the
    /// stream/non-stream choice in the path; Bedrock keeps `/v1/messages`.
    pub(crate) fn request_url(&self, model: &str, stream: bool) -> String {
        match self {
            Self::Bedrock { base_url, .. } => format!("{base_url}/v1/messages"),
            Self::Vertex {
                base_url,
                project,
                region,
            } => {
                let verb = if stream {
                    "streamRawPredict"
                } else {
                    "rawPredict"
                };
                let model = vertex_model_id(model);
                format!(
                    "{base_url}/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:{verb}"
                )
            }
        }
    }

    /// Rewrite a rendered Messages-API body for this gateway.
    pub(crate) fn adapt_body(&self, body: &mut Value) {
        let Some(map) = body.as_object_mut() else {
            return;
        };
        match self {
            Self::Bedrock { .. } => {
                if let Some(model) = map.get("model").and_then(Value::as_str) {
                    let mapped = bedrock_model_id(model);
                    map.insert("model".to_string(), Value::String(mapped));
                }
            }
            Self::Vertex { .. } => {
                map.remove("model");
                map.insert(
                    "anthropic_version".to_string(),
                    json!(VERTEX_ANTHROPIC_VERSION),
                );
            }
        }
    }

    /// Gateway auth headers, replacing the first-party auth chain entirely.
    /// `payload` is the exact serialized request body (`SigV4` signs over it),
    /// and `request_url` the full URL just built by [`Self::request_url`]
    /// (the host + path are extracted for the canonical request).
    pub(crate) async fn apply_auth(
        &self,
        builder: reqwest::RequestBuilder,
        request_url: &str,
        payload: &[u8],
    ) -> Result<reqwest::RequestBuilder, ApiError> {
        match self {
            Self::Bedrock {
                auth: BedrockAuth::Bearer(token),
                ..
            } => Ok(builder.header("x-api-key", token.as_str())),
            Self::Bedrock {
                auth:
                    BedrockAuth::SigV4 {
                        credentials,
                        region,
                    },
                ..
            } => {
                let (host, path) = split_host_path(request_url).ok_or_else(|| {
                    ApiError::Auth(format!(
                        "Bedrock gateway: malformed request URL `{request_url}`"
                    ))
                })?;
                let mut builder = builder;
                for (name, value) in super::aws_sigv4::sign_request(
                    credentials,
                    region,
                    "bedrock",
                    &host,
                    &path,
                    payload,
                    SystemTime::now(),
                ) {
                    builder = builder.header(name, value);
                }
                Ok(builder)
            }
            Self::Vertex { .. } => {
                let token = super::google_auth::vertex_access_token().await?;
                Ok(builder.bearer_auth(token))
            }
        }
    }
}

/// Extract `(host, path)` from an `https://host/path...` URL for the `SigV4`
/// canonical request (query strings are not used by the Messages endpoint).
fn split_host_path(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = match rest.find('/') {
        Some(index) => (rest[..index].to_string(), rest[index..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    };
    let path = path.split(['?', '#']).next().unwrap_or("/").to_string();
    Some((host, path))
}

/// `claude-opus-4-8` → `anthropic.claude-opus-4-8`;
/// `claude-haiku-4-5-20251001` → `anthropic.claude-haiku-4-5` (the Bedrock
/// Messages endpoint lists ids without date suffixes). Ids that already carry
/// a provider/region prefix (`anthropic.`, `us.`, `global.`, …) pass through.
fn bedrock_model_id(model: &str) -> String {
    if model.split('.').count() > 1 {
        return model.to_string();
    }
    format!("anthropic.{}", strip_date_suffix(model))
}

/// `claude-sonnet-4-5-20250929` → `claude-sonnet-4-5@20250929` (Vertex keeps
/// dated ids with an `@`); undated ids (`claude-opus-4-8`) and ids already in
/// `@` form pass through.
fn vertex_model_id(model: &str) -> String {
    if model.contains('@') {
        return model.to_string();
    }
    match split_date_suffix(model) {
        Some((base, date)) => format!("{base}@{date}"),
        None => model.to_string(),
    }
}

fn strip_date_suffix(model: &str) -> &str {
    split_date_suffix(model).map_or(model, |(base, _)| base)
}

/// `name-YYYYMMDD` → `(name, YYYYMMDD)`.
fn split_date_suffix(model: &str) -> Option<(&str, &str)> {
    let (base, candidate) = model.rsplit_once('-')?;
    (candidate.len() == 8 && candidate.bytes().all(|b| b.is_ascii_digit()))
        .then_some((base, candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn no_gateway_without_env_flags() {
        assert_eq!(resolve_from_lookup(&lookup_from(&[])), None);
        // "0"/"false" must not enable (CC semantics).
        let lookup = lookup_from(&[("CLAUDE_CODE_USE_BEDROCK", "0")]);
        assert_eq!(resolve_from_lookup(&lookup), None);
    }

    #[test]
    fn bedrock_resolves_default_endpoint_and_requires_bearer_token() {
        let lookup = lookup_from(&[("CLAUDE_CODE_USE_BEDROCK", "1")]);
        let error = resolve_from_lookup(&lookup)
            .expect("gateway requested")
            .unwrap_err();
        assert!(error.contains("AWS_BEARER_TOKEN_BEDROCK"), "{error}");

        let lookup = lookup_from(&[
            ("ZO_USE_BEDROCK", "1"),
            ("AWS_BEARER_TOKEN_BEDROCK", "tok"),
            ("AWS_REGION", "ap-northeast-2"),
        ]);
        let gateway = resolve_from_lookup(&lookup)
            .expect("requested")
            .expect("resolved");
        assert_eq!(
            gateway,
            CloudGateway::Bedrock {
                base_url: "https://bedrock-mantle.ap-northeast-2.api.aws/anthropic".to_string(),
                auth: BedrockAuth::Bearer("tok".to_string()),
            }
        );
        assert_eq!(
            gateway.request_url("claude-opus-4-8", true),
            "https://bedrock-mantle.ap-northeast-2.api.aws/anthropic/v1/messages"
        );
    }

    #[test]
    fn vertex_builds_model_url_and_body_version() {
        let lookup = lookup_from(&[
            ("CLAUDE_CODE_USE_VERTEX", "true"),
            ("ANTHROPIC_VERTEX_PROJECT_ID", "proj-1"),
        ]);
        let gateway = resolve_from_lookup(&lookup)
            .expect("requested")
            .expect("resolved");
        assert_eq!(
            gateway.request_url("claude-opus-4-8", true),
            "https://global-aiplatform.googleapis.com/v1/projects/proj-1/locations/global/publishers/anthropic/models/claude-opus-4-8:streamRawPredict"
        );
        assert_eq!(
            gateway.request_url("claude-sonnet-4-5-20250929", false),
            "https://global-aiplatform.googleapis.com/v1/projects/proj-1/locations/global/publishers/anthropic/models/claude-sonnet-4-5@20250929:rawPredict"
        );

        let mut body = serde_json::json!({"model": "claude-opus-4-8", "max_tokens": 10});
        gateway.adapt_body(&mut body);
        assert!(body.get("model").is_none(), "model moves into the URL");
        assert_eq!(body["anthropic_version"], VERTEX_ANTHROPIC_VERSION);
    }

    #[test]
    fn bedrock_body_maps_model_ids() {
        let gateway = CloudGateway::Bedrock {
            base_url: "https://x".to_string(),
            auth: BedrockAuth::Bearer("t".to_string()),
        };
        let mut body = serde_json::json!({"model": "claude-haiku-4-5-20251001"});
        gateway.adapt_body(&mut body);
        assert_eq!(body["model"], "anthropic.claude-haiku-4-5");

        // Already-prefixed ids (regional/global inference profiles) pass through.
        let mut body = serde_json::json!({"model": "global.anthropic.claude-opus-4-6-v1"});
        gateway.adapt_body(&mut body);
        assert_eq!(body["model"], "global.anthropic.claude-opus-4-6-v1");
    }

    #[test]
    fn enabling_both_gateways_is_a_configuration_error() {
        let lookup = lookup_from(&[
            ("ZO_USE_BEDROCK", "1"),
            ("ZO_USE_VERTEX", "1"),
            ("AWS_BEARER_TOKEN_BEDROCK", "tok"),
            ("ANTHROPIC_VERTEX_PROJECT_ID", "p"),
        ]);
        let error = resolve_from_lookup(&lookup)
            .expect("requested")
            .unwrap_err();
        assert!(error.contains("only one"), "{error}");
    }

    #[test]
    fn model_id_mapping_rules() {
        assert_eq!(
            bedrock_model_id("claude-opus-4-8"),
            "anthropic.claude-opus-4-8"
        );
        assert_eq!(
            bedrock_model_id("claude-sonnet-4-5-20250929"),
            "anthropic.claude-sonnet-4-5"
        );
        assert_eq!(
            bedrock_model_id("anthropic.claude-fable-5"),
            "anthropic.claude-fable-5"
        );
        assert_eq!(vertex_model_id("claude-opus-4-8"), "claude-opus-4-8");
        assert_eq!(
            vertex_model_id("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5@20251001"
        );
        assert_eq!(
            vertex_model_id("claude-sonnet-4-5@20250929"),
            "claude-sonnet-4-5@20250929"
        );
        // Version-ish tails that are not dates stay untouched.
        assert_eq!(vertex_model_id("claude-fable-5"), "claude-fable-5");
    }
}
