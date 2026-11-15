//! Confirmed endpoint behavior, stored independently for each URL and model.
//! A missing, expired or unreadable record leaves the current wire behavior alone.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// A learned answer is usable for seven days after its last observation.
pub const CAPABILITY_TTL_SECS: u64 = 7 * 24 * 60 * 60;

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
    at: u64,
    value: T,
}

impl<T> Observation<T> {
    fn fresh(&self, now: u64) -> bool {
        now.checked_sub(self.at).is_some_and(|age| age < CAPABILITY_TTL_SECS)
    }
}

/// Actual serialized body sizes observed at this endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestBodyBounds {
    /// Smallest body the endpoint rejected with HTTP 413.
    pub rejected_bytes: Option<u64>,
    /// Largest body whose HTTP request succeeded.
    pub largest_accepted_bytes: Option<u64>,
}

impl RequestBodyBounds {
    /// A confirmed successful size, or 75% of a rejected size. No refusal means
    /// no learned ceiling: an ordinary successful request is not a size limit.
    #[must_use]
    pub fn ceiling(self) -> Option<u64> {
        let rejected = self.rejected_bytes?;
        let ceiling = self.largest_accepted_bytes.filter(|&bytes| bytes > 0)
            .unwrap_or(rejected / 4 * 3 + rejected % 4 * 3 / 4);
        (ceiling > 0).then_some(ceiling)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    reasoning: Option<Observation<ReasoningAnswer>>,
    body: Option<Observation<RequestBodyBounds>>,
}

impl Record {
    fn expire(&mut self, now: u64) {
        if self.reasoning.as_ref().is_some_and(|entry| !entry.fresh(now)) { self.reasoning = None; }
        if self.body.as_ref().is_some_and(|entry| !entry.fresh(now)) { self.body = None; }
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn home() -> PathBuf {
    #[cfg(not(test))]
    { core_types::paths::default_config_home() }
    #[cfg(test)]
    {
        // Unit-test clients never read or write the person's capability cache.
        static TEST_HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        TEST_HOME.get_or_init(|| tempfile::tempdir().expect("test capability home")).path().to_path_buf()
    }
}

fn path_at(home: &Path, endpoint: &str, model: &str) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update(endpoint.trim().trim_end_matches('/').as_bytes());
    hash.update([0]);
    hash.update(model.trim().to_ascii_lowercase().as_bytes());
    home.join("cache").join("endpoint-capabilities").join(format!("{:x}.json", hash.finalize()))
}

fn load_at(path: &Path, clock: u64) -> Record {
    let mut record = fs::read(path).ok().filter(|bytes| bytes.len() <= 16 * 1024)
        .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok()).unwrap_or_default();
    record.expire(clock);
    record
}

fn update_at(path: &Path, clock: u64, change: impl FnOnce(&mut Record)) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::other("capability record has no parent"))?;
    fs::create_dir_all(parent)?;
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut record = load_at(path, clock);
    change(&mut record);
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    let temporary = path.with_extension(format!("tmp-{}-{}", std::process::id(), TEMP_SERIAL.fetch_add(1, Ordering::Relaxed)));
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
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result
}

pub(crate) fn reasoning_answer(endpoint: &str, model: &str) -> Option<ReasoningAnswer> {
    load_at(&path_at(&home(), endpoint, model), now()).reasoning.map(|entry| entry.value)
}

pub(crate) fn note_reasoning_answer(endpoint: &str, model: &str, answer: Option<ReasoningAnswer>) {
    let clock = now();
    let _ = update_at(&path_at(&home(), endpoint, model), clock, |record| {
        record.reasoning = answer.map(|value| Observation { at: clock, value });
    });
}

/// The fresh bounds for this exact endpoint and model, when any were observed.
#[must_use]
pub fn request_body_bounds(endpoint: &str, model: &str) -> Option<RequestBodyBounds> {
    load_at(&path_at(&home(), endpoint, model), now()).body.map(|entry| entry.value)
}

/// A local byte-budget refusal. It is never stored as a server observation.
pub(crate) fn request_too_large_error(payload_len: usize, limit: u64) -> Option<crate::ApiError> {
    let measured = payload_len as u64;
    if measured <= limit { return None; }
    Some(crate::ApiError::Api {
        status: reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        error_type: Some("request_too_large".to_string()),
        message: Some(format!("Request exceeds the maximum size: {measured} bytes serialized, request budget is {limit}")),
        body: String::new(), retryable: false, retry_after: None,
    })
}

/// Record only an HTTP success or 413. Rate limits, server errors and transport
/// failures say nothing about the endpoint's body limit.
pub(crate) fn note_http_status(endpoint: &str, model: &str, status: reqwest::StatusCode, bytes: usize) {
    if !status.is_success() && status != reqwest::StatusCode::PAYLOAD_TOO_LARGE { return; }
    let Ok(bytes) = u64::try_from(bytes) else { return; };
    let clock = now();
    let _ = update_at(&path_at(&home(), endpoint, model), clock, |record| {
        let mut value = record.body.as_ref().map_or_else(RequestBodyBounds::default, |entry| entry.value);
        if status.is_success() {
            value.largest_accepted_bytes = Some(value.largest_accepted_bytes.unwrap_or(0).max(bytes));
        } else {
            value.rejected_bytes = Some(value.rejected_bytes.map_or(bytes, |old| old.min(bytes)));
            if value.largest_accepted_bytes.is_some_and(|accepted| accepted >= bytes) {
                value.largest_accepted_bytes = None;
            }
        }
        record.body = Some(Observation { at: clock, value });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_and_corrupt_records_have_no_effect() {
        let home = tempfile::tempdir().unwrap();
        let path = path_at(home.path(), "https://acme.invalid/v1", "model-1");
        update_at(&path, 100, |record| {
            record.reasoning = Some(Observation { at: 100, value: ReasoningAnswer::Refused });
        }).unwrap();
        assert_eq!(load_at(&path, 100 + CAPABILITY_TTL_SECS - 1).reasoning.unwrap().value, ReasoningAnswer::Refused);
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
            record.body = Some(Observation { at: 100, value: RequestBodyBounds { rejected_bytes: Some(1000), largest_accepted_bytes: Some(600) } });
        }).unwrap();
        assert_eq!(load_at(&first, 101).body.unwrap().value.ceiling(), Some(600));
        assert!(load_at(&endpoint, 101).body.is_none());
        assert!(load_at(&model, 101).body.is_none());
        assert_eq!(path_at(home.path(), "https://acme.invalid/v1/", "MODEL-1"), first);
        assert!(fs::read_dir(first.parent().unwrap()).unwrap().all(|entry| !entry.unwrap().path().extension().unwrap().to_string_lossy().starts_with("tmp-")));
    }

    #[test]
    fn declarations_win_over_observations_at_each_scope() {
        struct Restore {
            providers: Option<std::ffi::OsString>,
            catalog: Option<std::ffi::OsString>,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in [(crate::CUSTOM_PROVIDERS_ENV, &self.providers), (crate::MODEL_CONTEXT_WINDOWS_ENV, &self.catalog)] {
                    match value { Some(value) => std::env::set_var(key, value), None => std::env::remove_var(key) }
                }
                crate::refresh_custom_providers_from_env();
            }
        }
        let _lock = crate::test_env_lock();
        let _restore = Restore { providers: std::env::var_os(crate::CUSTOM_PROVIDERS_ENV), catalog: std::env::var_os(crate::MODEL_CONTEXT_WINDOWS_ENV) };
        std::env::remove_var(crate::MODEL_CONTEXT_WINDOWS_ENV);
        let endpoint = "https://body-ceiling.invalid/v1";
        let model = "acme-byte-ceiling";
        let declare = |provider_limit: Option<u64>, model_limit: Option<u64>| {
            let model = model_limit.map_or_else(|| serde_json::json!(model), |limit| serde_json::json!({"id":model,"max_request_bytes":limit}));
            std::env::set_var(crate::CUSTOM_PROVIDERS_ENV, serde_json::json!([{
                "name":"acme", "base_url":endpoint, "requires_auth":false, "models":[model], "max_request_bytes":provider_limit
            }]).to_string());
            crate::refresh_custom_providers_from_env();
        };
        note_http_status(endpoint, model, reqwest::StatusCode::OK, 1200);
        note_http_status(endpoint, model, reqwest::StatusCode::PAYLOAD_TOO_LARGE, 4000);
        declare(None, None);
        assert_eq!(crate::max_request_bytes_for_model(model), Some(1200));
        declare(Some(2200), None);
        assert_eq!(crate::max_request_bytes_for_model(model), Some(2200));
        declare(Some(2200), Some(3300));
        assert_eq!(crate::max_request_bytes_for_model(model), Some(3300));
        declare(None, None);
        std::env::set_var(crate::MODEL_CONTEXT_WINDOWS_ENV, serde_json::json!({"models":[{"ids":[model],"max_request_bytes":4400}]}).to_string());
        assert_eq!(crate::max_request_bytes_for_model(model), Some(4400));
    }

    #[test]
    fn custom_declarations_include_provider_and_model_body_limits() {
        let declared: super::super::openai_compat::CustomProviderConfig = serde_json::from_str(
            r#"{"name":"acme","base_url":"https://acme.invalid/v1","max_request_bytes":1000,"models":["plain",{"id":"own","max_request_bytes":2000}]}"#
        ).unwrap();
        let provider = super::super::ResolvedCustomProvider::from(declared);
        assert_eq!(provider.max_request_bytes_for("plain"), Some(1000));
        assert_eq!(provider.max_request_bytes_for("own"), Some(2000));
    }

    #[test]
    fn rejection_without_a_confirmed_size_uses_seventy_five_percent() {
        assert_eq!(RequestBodyBounds { rejected_bytes: Some(1001), largest_accepted_bytes: None }.ceiling(), Some(750));
        assert_eq!(RequestBodyBounds { rejected_bytes: None, largest_accepted_bytes: Some(600) }.ceiling(), None);
    }
}
