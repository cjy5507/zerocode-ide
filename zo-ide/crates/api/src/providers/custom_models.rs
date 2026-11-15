//! Limits read from a person's OpenAI-compatible endpoint, below declarations.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::openai_compat::{ModelLimits, ModelListEntry, OpenAiCompatClient, OpenAiCompatConfig};
use super::{HarnessValueSource, ResolvedCustomProvider};

type EndpointModels = HashMap<String, HashMap<String, ModelLimits>>;

/// How long a failed read of one endpoint for one model is not tried again,
/// so a runtime rebuilt within it does not wait on the same failure.
const FAILED_READ_PAUSE: Duration = Duration::from_secs(5 * 60);

fn readings() -> &'static RwLock<EndpointModels> {
    static READINGS: OnceLock<RwLock<EndpointModels>> = OnceLock::new();
    READINGS.get_or_init(RwLock::default)
}

/// When each endpoint + model read last failed.
fn failed_reads() -> &'static Mutex<HashMap<String, Instant>> {
    static FAILED: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    FAILED.get_or_init(Mutex::default)
}

fn base_endpoint(base_url: &str) -> &str {
    let base = base_url.trim_end_matches('/');
    base.strip_suffix("/chat/completions").unwrap_or(base)
}

pub(super) fn models_endpoint(base_url: &str) -> String {
    format!("{}/models", base_endpoint(base_url))
}

/// Successful model lists are cached only for this process. A failed read
/// publishes nothing, leaving the existing declarations and fallbacks intact.
pub(super) fn remember(base_url: &str, entries: &[ModelListEntry]) {
    let models = entries.iter().map(|entry| {
        let fields = &entry.attributes;
        let positive = |key: &str| fields.get(key).and_then(Value::as_u64).filter(|&n| n > 0);
        let modalities = fields.get("input_modalities")
            .or_else(|| fields.get("architecture")?.get("input_modalities"))
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty() && items.iter().all(Value::is_string));
        let supports_vision = fields.get("supports_vision").and_then(Value::as_bool)
            .or_else(|| modalities.map(|items| items.iter().any(|item| {
                item.as_str().is_some_and(|kind| kind.eq_ignore_ascii_case("image"))
            })));
        let limits = ModelLimits {
            context_window: positive("context_length")
                .or_else(|| positive("max_model_len"))
                .or_else(|| positive("context_window")),
            max_output_tokens: positive("max_output_tokens")
                .or_else(|| positive("max_completion_tokens"))
                .or_else(|| fields.get("top_provider")?.get("max_completion_tokens")?
                    .as_u64().filter(|&n| n > 0)),
            max_request_bytes: None,
            supports_vision,
        };
        (entry.id.to_ascii_lowercase(), limits)
    }).collect();
    readings().write().unwrap_or_else(PoisonError::into_inner)
        .insert(base_endpoint(base_url).to_string(), models);
}

pub(super) fn read_limits(provider: &ResolvedCustomProvider, model: &str) -> ModelLimits {
    if !provider.declared_by_person { return ModelLimits::default(); }
    let Some(id) = provider.declared_id(model) else { return ModelLimits::default(); };
    readings().read().unwrap_or_else(PoisonError::into_inner)
        .get(base_endpoint(provider.config.default_base_url))
        .and_then(|models| models.get(&id.to_ascii_lowercase())).copied().unwrap_or_default()
}

/// What the endpoint of the person's provider that serves `model` said about
/// it. Never another endpoint's answer, and nothing for a built-in provider.
pub(super) fn selected_reading(model: &str) -> ModelLimits {
    // Most processes never read a list: then no provider lookup is paid here.
    if readings().read().unwrap_or_else(PoisonError::into_inner).is_empty() {
        return ModelLimits::default();
    }
    selected_provider(model)
        .map_or_else(ModelLimits::default, |(provider, wire)| read_limits(&provider, &wire))
}

pub(super) fn selected_provider(model: &str) -> Option<(Arc<ResolvedCustomProvider>, String)> {
    let wire = super::wire_model_id(model);
    let provider = super::custom_provider_for_model(model)
        .or_else(|| super::custom_provider_for_model(&wire))?;
    provider.declared_by_person.then_some((provider, wire))
}

/// Read the selected custom endpoint once per process before its runtime is
/// built. Built-in providers never take this path. Failure keeps today's
/// limits, and that endpoint is not asked again for that model for five
/// minutes; the request has a two-second timeout and performs no generation.
pub fn refresh_custom_model_capabilities(model: &str) {
    let Some((provider, wire)) = selected_provider(model) else { return; };
    let endpoint = base_endpoint(provider.config.default_base_url).to_string();
    if std::env::var_os("ZO_DISABLE_MODEL_DISCOVERY").is_some()
        || readings().read().unwrap_or_else(PoisonError::into_inner).contains_key(&endpoint)
    {
        return;
    }
    let attempt = format!("{endpoint} {}", wire.to_ascii_lowercase());
    if failed_reads().lock().unwrap_or_else(PoisonError::into_inner)
        .get(&attempt).is_some_and(|failed| failed.elapsed() < FAILED_READ_PAUSE)
    {
        return;
    }
    let read = run_read(move || async move {
        let client = if super::custom_provider_needs_key(&provider) {
            OpenAiCompatClient::from_env(provider.config)
        } else {
            OpenAiCompatClient::from_env_optional_auth(provider.config)
        };
        let Ok(client) = client else { return false; };
        client.read_model_capabilities().await.is_ok()
    });
    let mut failed = failed_reads().lock().unwrap_or_else(PoisonError::into_inner);
    if read {
        failed.remove(&attempt);
    } else {
        failed.insert(attempt, Instant::now());
    }
}

/// Drive one read to its end from synchronous code. The caller may itself be
/// inside a runtime, so the read is made and run on a thread of its own, on
/// the shared bridge runtime, which is never dropped. Dropping a runtime waits
/// for its blocking work, and a host-name lookup is blocking work that can
/// outlive the request's two-second timeout: left on a runtime that stays, it
/// no longer holds the session start.
fn run_read<M, F>(make_read: M) -> bool
where
    M: FnOnce() -> F + Send + 'static,
    F: Future<Output = bool>,
{
    std::thread::Builder::new()
        .name("zo-custom-models".to_string())
        .spawn(move || crate::sync_bridge::run_blocking(make_read()))
        .ok()
        .and_then(|reader| reader.join().ok())
        .unwrap_or(false)
}

pub(super) fn vision_for_config(config: &OpenAiCompatConfig, model: &str) -> Option<bool> {
    if !config.is_declared_by_the_person() { return None; }
    let wire = super::wire_model_id(model);
    super::custom_provider_snapshot().iter().find(|provider| {
        provider.declared_by_person
            && provider.named(config.provider_name)
            && provider.config.connection_id == config.connection_id
            && base_endpoint(provider.config.default_base_url) == base_endpoint(config.default_base_url)
            && provider.declared_id(&wire).is_some()
    }).and_then(|provider| provider.vision_for(&wire)).map(|vision| vision.value)
}

/// One capability and its evidence, kept together even when new fields are added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityValue<T> {
    pub value: T,
    pub source: HarnessValueSource,
}

/// The selected custom model's effective capabilities, each carrying its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomModelCapabilities {
    pub context_window: CapabilityValue<u64>,
    pub max_output_tokens: CapabilityValue<u32>,
    pub supports_vision: CapabilityValue<bool>,
}

/// Each value with the source the resolver itself found it in: a
/// declaration (the person's or the catalog's), a `/models` reading, or the
/// existing fallback. A reading is never a measured experiment.
#[must_use]
pub fn custom_model_capabilities(model: &str) -> Option<CustomModelCapabilities> {
    let (provider, wire) = selected_provider(model)?;
    Some(CustomModelCapabilities {
        context_window: super::context_window_with_source(model),
        max_output_tokens: super::max_tokens_with_source(model),
        supports_vision: provider.vision_for(&wire)
            .unwrap_or(CapabilityValue { value: false, source: HarnessValueSource::Default }),
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    /// reqwest resolves a host name as blocking work on the runtime. A lookup
    /// that hangs past the request's timeout (no VPN, a silent DNS server)
    /// must not hold the read: the read ends when its own timeout does.
    #[test]
    fn a_read_ends_at_its_timeout_while_a_name_lookup_still_hangs() {
        const LOOKUP: Duration = Duration::from_secs(10);
        const TIMEOUT: Duration = Duration::from_millis(100);
        let started = Instant::now();
        let read = super::run_read(|| async {
            drop(tokio::task::spawn_blocking(|| std::thread::sleep(LOOKUP)));
            tokio::time::sleep(TIMEOUT).await;
            false
        });
        let waited = started.elapsed();
        assert!(!read);
        assert!(waited < Duration::from_secs(5), "the read waited {waited:?} on a hung name lookup");
    }
}
