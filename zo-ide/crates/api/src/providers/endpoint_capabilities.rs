//! What one endpoint was seen to do, kept for each endpoint URL and model in
//! zo's home (`cache/endpoint-capabilities/<sha256>.json`), so the next session
//! starts from it instead of being refused again. Two things are kept:
//!
//! * how the endpoint answered `reasoning_content` ([`ReasoningAnswer`]);
//! * the request-body sizes it refused with HTTP 413 and, once it has refused
//!   one, the largest it took ([`RequestBodyBounds`]).
//!
//! An entry is good for [`CAPABILITY_TTL_SECS`] after the refusal that taught
//! it. A missing, expired or unreadable record leaves the wire as it was. Rate
//! limits, server errors and transport failures teach nothing. A record is
//! written with a temporary file renamed over it, under a lock file, so two
//! sessions never interleave their writes.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// A learned answer is usable for seven days after the refusal that taught it.
pub const CAPABILITY_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// The share of a refused body taken as the endpoint's ceiling, unless a larger
/// body is known to pass ([`ceiling_after_refusal`]).
const REFUSED_BODY_SHARE_PERCENT: u64 = 75;

static TEMP_SERIAL: AtomicU64 = AtomicU64::new(0);

/// How the endpoint answered a request carrying `reasoning_content`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReasoningAnswer {
    Refused,
    Demanded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Observation<T> {
    /// When this was learned, in seconds since the Unix epoch. For body sizes:
    /// when the endpoint last refused one — a success after it does not move
    /// it, so a refusal ages out even while the endpoint is in daily use.
    at: u64,
    value: T,
}

impl<T> Observation<T> {
    fn fresh(&self, now: u64) -> bool {
        now.checked_sub(self.at)
            .is_some_and(|age| age < CAPABILITY_TTL_SECS)
    }
}

/// Serialized body sizes, in bytes, this endpoint answered for this model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestBodyBounds {
    /// Smallest body the endpoint refused with HTTP 413.
    pub rejected_bytes: Option<u64>,
    /// Largest body it took, kept only below `rejected_bytes`.
    pub largest_accepted_bytes: Option<u64>,
}

impl RequestBodyBounds {
    /// The ceiling a body is planned under: the larger of the largest size
    /// known to pass and [`ceiling_after_refusal`] of the refused size — a
    /// success smaller than that share says nothing the refusal does not. No
    /// refusal, no ceiling: an ordinary successful request says nothing about
    /// a limit.
    #[must_use]
    pub fn ceiling(self) -> Option<u64> {
        let rejected = self.rejected_bytes?;
        let ceiling = self
            .largest_accepted_bytes
            .unwrap_or(0)
            .max(ceiling_after_refusal(rejected));
        (ceiling > 0).then_some(ceiling)
    }

    /// The size the exact body must stay under before it is sent: the refused
    /// size, unless a body at least that large has passed since.
    fn refused_limit(self) -> Option<u64> {
        let rejected = self.rejected_bytes?;
        self.largest_accepted_bytes
            .is_none_or(|accepted| accepted < rejected)
            .then_some(rejected)
    }
}

/// The ceiling one refusal of `refused_bytes` teaches by itself: three
/// quarters of it, the least the endpoint record plans under after a refusal
/// (`RequestBodyBounds::ceiling`). Shared with the runtime's answer to a
/// refusal nothing was recorded for, so both plan a retry the same way.
#[must_use]
pub fn ceiling_after_refusal(refused_bytes: u64) -> u64 {
    refused_bytes / 100 * REFUSED_BODY_SHARE_PERCENT
        + refused_bytes % 100 * REFUSED_BODY_SHARE_PERCENT / 100
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    reasoning: Option<Observation<ReasoningAnswer>>,
    body: Option<Observation<RequestBodyBounds>>,
}

impl Record {
    fn expire(&mut self, now: u64) {
        if self.reasoning.as_ref().is_some_and(|entry| !entry.fresh(now)) {
            self.reasoning = None;
        }
        if self.body.as_ref().is_some_and(|entry| !entry.fresh(now)) {
            self.body = None;
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn home() -> PathBuf {
    #[cfg(not(test))]
    {
        core_types::paths::default_config_home()
    }
    #[cfg(test)]
    {
        // Unit-test clients never read or write the person's capability cache.
        static TEST_HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
        TEST_HOME
            .get_or_init(|| tempfile::tempdir().expect("test capability home"))
            .path()
            .to_path_buf()
    }
}

fn path_at(home: &Path, endpoint: &str, model: &str) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update(endpoint.trim().trim_end_matches('/').as_bytes());
    hash.update([0]);
    hash.update(model.trim().to_ascii_lowercase().as_bytes());
    home.join("cache")
        .join("endpoint-capabilities")
        .join(format!("{:x}.json", hash.finalize()))
}

fn load_at(path: &Path, clock: u64) -> Record {
    let mut record = fs::read(path)
        .ok()
        .filter(|bytes| bytes.len() <= 16 * 1024)
        .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
        .unwrap_or_default();
    record.expire(clock);
    record
}

/// Read the record under its lock, let `change` edit it, and write it back
/// only when `change` says it changed something.
fn update_at(path: &Path, clock: u64, change: impl FnOnce(&mut Record) -> bool) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("capability record has no parent"))?;
    fs::create_dir_all(parent)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut record = load_at(path, clock);
    if !change(&mut record) {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        TEMP_SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn reasoning_answer(endpoint: &str, model: &str) -> Option<ReasoningAnswer> {
    load_at(&path_at(&home(), endpoint, model), now())
        .reasoning
        .map(|entry| entry.value)
}

/// Keep what this endpoint answered about `reasoning_content`, or forget it.
pub(crate) fn note_reasoning_answer(endpoint: &str, model: &str, answer: Option<ReasoningAnswer>) {
    let clock = now();
    let _ = update_at(&path_at(&home(), endpoint, model), clock, |record| {
        if record.reasoning.as_ref().map(|entry| entry.value) == answer {
            return false;
        }
        record.reasoning = answer.map(|value| Observation { at: clock, value });
        true
    });
}

/// The fresh bounds for this exact endpoint and model, when any were observed.
#[must_use]
pub fn request_body_bounds(endpoint: &str, model: &str) -> Option<RequestBodyBounds> {
    load_at(&path_at(&home(), endpoint, model), now())
        .body
        .map(|entry| entry.value)
}

/// Refuse a body here, before it is uploaded, when it is over the declared
/// ceiling or no smaller than a size this endpoint refused. The error is shaped
/// like the endpoint's own 413, so it reaches the same recovery, and it is
/// never stored as something the endpoint said.
pub(crate) fn preflight(
    payload_len: usize,
    declared: Option<u64>,
    bounds: Option<RequestBodyBounds>,
) -> Option<crate::ApiError> {
    let measured = payload_len as u64;
    let message = if let Some(limit) = declared {
        (measured > limit).then(|| {
            format!(
                "Request exceeds the maximum size: {measured} bytes serialized, endpoint accepts {limit}"
            )
        })?
    } else {
        let refused = bounds.and_then(RequestBodyBounds::refused_limit)?;
        (measured >= refused).then(|| {
            format!(
                "Request exceeds the maximum size: {measured} bytes serialized, endpoint refused {refused}"
            )
        })?
    };
    Some(crate::ApiError::Api {
        status: reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        error_type: Some("request_too_large".to_string()),
        message: Some(message),
        body: String::new(),
        retryable: false,
        retry_after: None,
    })
}

/// The largest body each record's endpoint took in this process before it
/// refused one. A success says nothing about a limit until a refusal does, so
/// it is kept here and costs no disk write; the first 413 writes it down as
/// the largest size known to pass.
fn accepted_in_memory() -> &'static Mutex<HashMap<PathBuf, u64>> {
    static ACCEPTED: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();
    ACCEPTED.get_or_init(Mutex::default)
}

/// Note what the endpoint answered a body of `bytes`. Only an HTTP success or
/// a 413 counts. `known` is the record read before the body was sent: a
/// success is written only after a refusal — when it is larger than the
/// largest the record holds, or as large as the refused body, which ends that
/// refusal — so an endpoint that never refuses a body costs no write at all.
pub(crate) fn note_http_status(
    endpoint: &str,
    model: &str,
    status: reqwest::StatusCode,
    bytes: usize,
    known: Option<RequestBodyBounds>,
) {
    let Ok(bytes) = u64::try_from(bytes) else {
        return;
    };
    let path = path_at(&home(), endpoint, model);
    if status.is_success() {
        {
            let mut accepted = accepted_in_memory()
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let largest = accepted.entry(path.clone()).or_default();
            *largest = (*largest).max(bytes);
        }
        // Worth a look only after a refusal: a larger size taken than the
        // record holds, or a body as large as the refused one taken after all.
        let news = known.is_some_and(|bounds| {
            bounds.rejected_bytes.is_some_and(|rejected| {
                bytes >= rejected
                    || bounds
                        .largest_accepted_bytes
                        .is_none_or(|accepted| bytes > accepted)
            })
        });
        if news {
            let _ = update_at(&path, now(), |record| {
                let Some(entry) = record.body.as_mut() else {
                    return false;
                };
                let Some(rejected) = entry.value.rejected_bytes else {
                    return false;
                };
                if bytes >= rejected {
                    // The endpoint took a body as large as one it refused:
                    // that refusal no longer holds.
                    record.body = None;
                    return true;
                }
                if entry
                    .value
                    .largest_accepted_bytes
                    .is_some_and(|accepted| accepted >= bytes)
                {
                    return false;
                }
                entry.value.largest_accepted_bytes = Some(bytes);
                true
            });
        }
        return;
    }
    if status != reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        return;
    }
    let accepted_before = accepted_in_memory()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&path)
        .copied();
    let clock = now();
    let _ = update_at(&path, clock, |record| {
        let mut bounds = record
            .body
            .as_ref()
            .map_or_else(RequestBodyBounds::default, |entry| entry.value);
        let rejected = bounds.rejected_bytes.map_or(bytes, |old| old.min(bytes));
        bounds.rejected_bytes = Some(rejected);
        bounds.largest_accepted_bytes = bounds
            .largest_accepted_bytes
            .into_iter()
            .chain(accepted_before)
            .filter(|&accepted| accepted < rejected)
            .max();
        record.body = Some(Observation { at: clock, value: bounds });
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Puts the person's providers and model catalog back as they were.
    struct Restore {
        providers: Option<std::ffi::OsString>,
        catalog: Option<std::ffi::OsString>,
    }

    impl Restore {
        fn save() -> Self {
            Self {
                providers: std::env::var_os(crate::CUSTOM_PROVIDERS_ENV),
                catalog: std::env::var_os(crate::MODEL_CONTEXT_WINDOWS_ENV),
            }
        }
    }

    impl Drop for Restore {
        fn drop(&mut self) {
            for (key, value) in [
                (crate::CUSTOM_PROVIDERS_ENV, &self.providers),
                (crate::MODEL_CONTEXT_WINDOWS_ENV, &self.catalog),
            ] {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            crate::refresh_custom_providers_from_env();
        }
    }

    fn bounds(rejected: u64, accepted: Option<u64>) -> RequestBodyBounds {
        RequestBodyBounds {
            rejected_bytes: Some(rejected),
            largest_accepted_bytes: accepted,
        }
    }

    #[test]
    fn expired_and_corrupt_records_have_no_effect() {
        let home = tempfile::tempdir().unwrap();
        let path = path_at(home.path(), "https://acme.invalid/v1", "model-1");
        update_at(&path, 100, |record| {
            record.reasoning = Some(Observation {
                at: 100,
                value: ReasoningAnswer::Refused,
            });
            true
        })
        .unwrap();
        assert_eq!(
            load_at(&path, 100 + CAPABILITY_TTL_SECS - 1).reasoning.unwrap().value,
            ReasoningAnswer::Refused
        );
        assert!(load_at(&path, 100 + CAPABILITY_TTL_SECS).reasoning.is_none());
        assert!(load_at(&path, 99).reasoning.is_none(), "a future timestamp is not evidence");
        fs::write(&path, "partial JSON").unwrap();
        assert!(load_at(&path, 100).reasoning.is_none());
    }

    #[test]
    fn records_survive_reopening_and_do_not_mix_endpoints_or_models() {
        let home = tempfile::tempdir().unwrap();
        let first = path_at(home.path(), "https://acme.invalid/v1", "model-1");
        let endpoint = path_at(home.path(), "https://other.invalid/v1", "model-1");
        let model = path_at(home.path(), "https://acme.invalid/v1", "model-2");
        assert_ne!(first, endpoint);
        assert_ne!(first, model);
        update_at(&first, 100, |record| {
            record.body = Some(Observation {
                at: 100,
                value: bounds(1000, Some(800)),
            });
            true
        })
        .unwrap();
        assert_eq!(load_at(&first, 101).body.unwrap().value.ceiling(), Some(800));
        assert!(load_at(&endpoint, 101).body.is_none());
        assert!(load_at(&model, 101).body.is_none());
        assert_eq!(path_at(home.path(), "https://acme.invalid/v1/", "MODEL-1"), first);
        assert!(fs::read_dir(first.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .path()
                .extension()
                .unwrap()
                .to_string_lossy()
                .starts_with("tmp-")
        }));
    }

    #[test]
    fn a_success_before_any_refusal_writes_nothing_and_the_first_refusal_keeps_it() {
        let (endpoint, model) = ("https://writes-on-refusal.invalid/v1", "acme-quiet");
        let path = path_at(&home(), endpoint, model);
        note_http_status(endpoint, model, reqwest::StatusCode::OK, 500, None);
        note_http_status(endpoint, model, reqwest::StatusCode::OK, 900, None);
        assert!(!path.exists(), "an endpoint that never refused costs no write");

        note_http_status(endpoint, model, reqwest::StatusCode::PAYLOAD_TOO_LARGE, 2000, None);
        let known = request_body_bounds(endpoint, model);
        assert_eq!(known, Some(bounds(2000, Some(900))), "the largest size taken before it");

        // After the refusal a larger success is written, a smaller one is not.
        note_http_status(endpoint, model, reqwest::StatusCode::OK, 1200, known);
        let known = request_body_bounds(endpoint, model);
        assert_eq!(known, Some(bounds(2000, Some(1200))));
        let written = fs::read(&path).unwrap();
        note_http_status(endpoint, model, reqwest::StatusCode::OK, 1000, known);
        assert_eq!(fs::read(&path).unwrap(), written, "nothing new, nothing written");

        // A body as large as the refused one passed: that refusal no longer holds.
        note_http_status(endpoint, model, reqwest::StatusCode::OK, 2100, known);
        assert_eq!(request_body_bounds(endpoint, model), None);
    }

    #[test]
    fn a_refusal_ages_out_seven_days_after_it_was_taught() {
        let (endpoint, model) = ("https://ages-out.invalid/v1", "acme-aging");
        let path = path_at(&home(), endpoint, model);
        let taught = now() - CAPABILITY_TTL_SECS + 600;
        update_at(&path, taught, |record| {
            record.body = Some(Observation {
                at: taught,
                value: bounds(2000, None),
            });
            true
        })
        .unwrap();
        let known = request_body_bounds(endpoint, model);
        assert_eq!(known, Some(bounds(2000, None)));
        note_http_status(endpoint, model, reqwest::StatusCode::OK, 1500, known);
        let record = load_at(&path, taught);
        assert_eq!(
            record.body.map(|entry| (entry.at, entry.value)),
            Some((taught, bounds(2000, Some(1500)))),
            "a success after the refusal does not make it younger"
        );
        assert!(load_at(&path, taught + CAPABILITY_TTL_SECS).body.is_none());
    }

    #[test]
    fn rate_limits_server_errors_and_other_refusals_teach_nothing() {
        let (endpoint, model) = ("https://teaches-nothing.invalid/v1", "acme-busy");
        for status in [
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            reqwest::StatusCode::BAD_REQUEST,
        ] {
            note_http_status(endpoint, model, status, 5000, None);
        }
        assert!(!path_at(&home(), endpoint, model).exists());
        assert_eq!(request_body_bounds(endpoint, model), None);
    }

    #[test]
    fn the_preflight_refuses_over_a_declaration_or_at_a_refused_size() {
        let learnt = Some(bounds(2000, Some(1500)));
        assert!(preflight(999, Some(999), None).is_none(), "the declared limit is inclusive");
        let declared = preflight(1000, Some(999), learnt).expect("over the declaration");
        assert!(declared.to_string().contains("endpoint accepts 999"), "{declared}");
        assert!(preflight(3000, Some(5000), learnt).is_none(), "a declaration wins over a refusal");
        assert!(preflight(1999, None, learnt).is_none(), "larger than a size taken is no limit");
        let refused = preflight(2000, None, learnt).expect("at the refused size");
        assert!(refused.to_string().contains("endpoint refused 2000"), "{refused}");
        assert_eq!(refused.provider_error_class(), crate::ProviderErrorClass::ContextOverflow);
        assert!(!refused.is_retryable(), "the same body would be refused again");
        assert!(preflight(9000, None, Some(bounds(2000, Some(2500)))).is_none(), "a larger body passed since");
        assert!(preflight(usize::MAX / 2, None, None).is_none(), "nothing declared, nothing refused");
    }

    #[test]
    fn declarations_win_over_observations_at_each_scope() {
        let _lock = crate::test_env_lock();
        let _restore = Restore::save();
        std::env::remove_var(crate::MODEL_CONTEXT_WINDOWS_ENV);
        let endpoint = "https://body-ceiling.invalid/v1";
        let model = "acme-byte-ceiling";
        let declare = |provider_limit: Option<u64>, model_limit: Option<u64>| {
            let model = model_limit.map_or_else(
                || serde_json::json!(model),
                |limit| serde_json::json!({"id": model, "max_request_bytes": limit}),
            );
            std::env::set_var(
                crate::CUSTOM_PROVIDERS_ENV,
                serde_json::json!([{
                    "name": "acme", "base_url": endpoint, "requires_auth": false,
                    "models": [model], "max_request_bytes": provider_limit
                }])
                .to_string(),
            );
            crate::refresh_custom_providers_from_env();
        };
        note_http_status(endpoint, model, reqwest::StatusCode::OK, 1200, None);
        note_http_status(endpoint, model, reqwest::StatusCode::PAYLOAD_TOO_LARGE, 4000, None);
        declare(None, None);
        // Learnt: the larger of 1,200 taken and three quarters of 4,000 refused.
        assert_eq!(crate::max_request_bytes_for_model(model), Some(3000));
        declare(Some(2200), None);
        assert_eq!(crate::max_request_bytes_for_model(model), Some(2200));
        declare(Some(2200), Some(3300));
        assert_eq!(crate::max_request_bytes_for_model(model), Some(3300));
        declare(None, None);
        std::env::set_var(
            crate::MODEL_CONTEXT_WINDOWS_ENV,
            serde_json::json!({"models": [{"ids": [model], "max_request_bytes": 4400}]}).to_string(),
        );
        assert_eq!(crate::max_request_bytes_for_model(model), Some(4400));
    }

    #[test]
    fn custom_declarations_include_provider_and_model_body_limits() {
        let declared: super::super::openai_compat::CustomProviderConfig = serde_json::from_str(
            r#"{"name":"acme","base_url":"https://acme.invalid/v1","max_request_bytes":1000,"models":["plain",{"id":"own","max_request_bytes":2000}]}"#,
        )
        .unwrap();
        let provider = super::super::ResolvedCustomProvider::from(declared);
        assert_eq!(provider.max_request_bytes_for("plain"), Some(1000));
        assert_eq!(provider.max_request_bytes_for("own"), Some(2000));
    }

    #[test]
    fn the_learnt_ceiling_is_the_larger_of_a_size_taken_and_three_quarters_of_the_refusal() {
        assert_eq!(bounds(1000, None).ceiling(), Some(750), "nothing taken: three quarters");
        assert_eq!(bounds(1000, Some(600)).ceiling(), Some(750), "a smaller success: three quarters");
        assert_eq!(bounds(1000, Some(900)).ceiling(), Some(900), "a larger success: that size");
        assert_eq!(bounds(1001, None).ceiling(), Some(750));
        assert_eq!(ceiling_after_refusal(1001), 750);
        assert_eq!(ceiling_after_refusal(u64::MAX), u64::MAX / 100 * 75 + 11);
        assert_eq!(
            RequestBodyBounds {
                rejected_bytes: None,
                largest_accepted_bytes: Some(600)
            }
            .ceiling(),
            None
        );
    }

    /// v1.1.155's ceiling for `model`: the catalog alone.
    fn v155_max_request_bytes(model: &str) -> Option<u64> {
        let canonical_lower = super::super::resolve_model_alias(model).to_ascii_lowercase();
        super::super::catalog_fact(|catalog| catalog.max_request_bytes_for(model, &canonical_lower))
    }

    #[test]
    fn shipped_rows_and_aliases_keep_their_v155_ceilings_with_nothing_learnt() {
        let _lock = crate::test_env_lock();
        let _restore = Restore::save();
        std::env::remove_var(crate::MODEL_CONTEXT_WINDOWS_ENV);
        std::env::set_var(crate::CUSTOM_PROVIDERS_ENV, "[]");
        crate::refresh_custom_providers_from_env();
        let catalog = super::super::builtin_model_context_catalog();
        let mut ids: Vec<&str> = catalog
            .models
            .iter()
            .flat_map(|row| row.ids.iter().map(String::as_str))
            .collect();
        ids.extend(catalog.aliases.iter().map(|row| row.alias.as_str()));
        assert!(ids.len() >= 60, "the shipped rows and aliases: {}", ids.len());
        let mut declared = 0;
        for id in ids {
            let ceiling = crate::max_request_bytes_for_model(id);
            assert_eq!(ceiling, v155_max_request_bytes(id), "{id}");
            declared += usize::from(ceiling.is_some());
        }
        assert!(declared > 0, "the Anthropic rows declare one");
        assert_eq!(crate::max_request_bytes_for_model("claude-opus-5-5"), Some(33_554_432));
        for undeclared in ["gpt-5.5", "deepseek-v4-flash", "gemini-3.6-flash", "o3"] {
            assert_eq!(crate::max_request_bytes_for_model(undeclared), None, "{undeclared}");
        }
    }

    #[test]
    fn a_built_in_provider_is_read_back_at_the_url_its_client_sends_to() {
        use super::super::openai_compat::OpenAiCompatConfig;
        use super::super::ProviderKind;
        for (kind, config) in [
            (ProviderKind::OpenAi, OpenAiCompatConfig::openai()),
            (ProviderKind::Xai, OpenAiCompatConfig::xai()),
            (ProviderKind::Google, OpenAiCompatConfig::google()),
            (ProviderKind::Ollama, OpenAiCompatConfig::ollama()),
        ] {
            let metadata = kind.metadata();
            assert_eq!(
                (metadata.base_url_env, metadata.default_base_url),
                (config.base_url_env, config.default_base_url),
                "{kind:?}"
            );
        }
        let anthropic = ProviderKind::Anthropic.metadata();
        assert_eq!(
            (anthropic.base_url_env, anthropic.default_base_url),
            ("ANTHROPIC_BASE_URL", super::super::anthropic::DEFAULT_BASE_URL)
        );
    }

    /// A local endpoint that refuses every body with 413 and keeps each
    /// body's size.
    async fn refusing_endpoint() -> (String, std::sync::Arc<Mutex<Vec<usize>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let sizes = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&sizes);
        tokio::spawn(async move {
            loop {
                let Ok((mut conn, _)) = listener.accept().await else {
                    return;
                };
                let mut bytes = Vec::new();
                let mut chunk = [0_u8; 8192];
                let body_len = loop {
                    let count = conn.read(&mut chunk).await.unwrap_or(0);
                    if count == 0 {
                        break None;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                    let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break Some(length);
                    }
                };
                let Some(body_len) = body_len.filter(|&length| length > 0) else {
                    continue;
                };
                seen.lock().unwrap().push(body_len);
                let refusal = r#"{"error":{"message":"request too large","type":"request_too_large"}}"#;
                let answer = format!(
                    "HTTP/1.1 413 Payload Too Large\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{refusal}",
                    refusal.len()
                );
                let _ = conn.write_all(answer.as_bytes()).await;
                let _ = conn.flush().await;
            }
        });
        (url, sizes)
    }

    #[test]
    fn a_refusal_the_product_client_records_is_read_back_for_the_picked_model() {
        let _lock = crate::test_env_lock();
        let _restore = Restore::save();
        std::env::remove_var(crate::MODEL_CONTEXT_WINDOWS_ENV);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (url, sizes) = runtime.block_on(refusing_endpoint());
        std::env::set_var(
            crate::CUSTOM_PROVIDERS_ENV,
            serde_json::json!([
                {
                    "name": "Acme Gateway", "base_url": url, "requires_auth": false,
                    "models": ["Acme-Coder-1", "Acme-Coder-2"]
                },
                {
                    "name": "Other Gateway", "base_url": "http://127.0.0.1:9/v1",
                    "requires_auth": false, "models": ["Acme-Coder-1"]
                }
            ])
            .to_string(),
        );
        crate::refresh_custom_providers_from_env();
        let picks = ["acme-coder-1", "ACME-CODER-1", "Acme Gateway/acme-coder-1"];
        for picked in picks {
            assert_eq!(crate::max_request_bytes_for_model(picked), None, "nothing learnt yet: {picked}");
        }
        // Built and sent the way a session builds and sends: the client for
        // the picked model, the model id the wire names.
        let picked = "Acme Gateway/acme-coder-1";
        let client = crate::ProviderClient::from_model(picked).expect("the person's provider");
        let request = crate::MessageRequest {
            model: crate::wire_model_id(picked),
            max_tokens: 16,
            messages: vec![crate::InputMessage::user_text("hello")],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        };
        assert!(runtime.block_on(client.send_message(&request)).is_err(), "refused");
        let sent = sizes.lock().unwrap().first().copied().expect("one body reached the endpoint");
        let sent = u64::try_from(sent).unwrap();
        for picked in picks {
            assert_eq!(
                crate::max_request_bytes_for_model(picked),
                Some(ceiling_after_refusal(sent)),
                "the record the client wrote is the one read for {picked}"
            );
        }
        // Another model at the same endpoint, the same model at another one.
        for elsewhere in ["acme-coder-2", "Other Gateway/acme-coder-1"] {
            assert_eq!(crate::max_request_bytes_for_model(elsewhere), None, "{elsewhere}");
        }
    }
}
