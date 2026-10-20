//! Per-project Computer call counts, without page or request text.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use runtime::ToolExecutionResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ToolContext, ToolError};

const FILE: &str = "computer-use-tally.json";
const FILE_CAP: u64 = 512 * 1024;
const BUNDLE_CAP: usize = 64;
const KIND_CAP: usize = 32;
const ROUTE_REFUSAL: &str = "browser_requires_work_browser";
static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Default)]
struct Receipt {
    bundle: Option<String>,
    several_bundles: bool,
    unknown_classification: bool,
    refusal: Option<String>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Receipt>> = const { RefCell::new(None) };
}

struct Collection(Option<Receipt>);

impl Drop for Collection {
    fn drop(&mut self) { ACTIVE.with(|active| *active.borrow_mut() = self.0.take()); }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct BundleCounts {
    calls: u64,
    refusals: BTreeMap<String, u64>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct Tally {
    since: u64,
    calls: u64,
    refusals: BTreeMap<String, u64>,
    by_bundle: BTreeMap<String, BundleCounts>,
    browser_route_refusals: u64,
    browser_chrome_calls: u64,
    browser_classification_unknown: u64,
}

fn is_kind(text: &str) -> bool {
    !text.is_empty() && text.len() <= 64 && text.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn kind(text: &str) -> &str { if is_kind(text) { text } else { "other" } }

fn is_bundle(text: &str) -> bool {
    matches!(text, "unknown" | "other") || (!text.is_empty() && text.len() <= 255 && text.contains('.')
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'!')))
}

fn bucket<T>(counts: &BTreeMap<String, T>, key: &str, cap: usize) -> String {
    if counts.contains_key(key) || counts.keys().filter(|key| key.as_str() != "other").count() < cap {
        key.to_string()
    } else { "other".to_string() }
}

fn increment(counts: &mut BTreeMap<String, u64>, code: &str) {
    let key = bucket(counts, kind(code), KIND_CAP);
    let count = counts.entry(key).or_default();
    *count = count.saturating_add(1);
}

impl Tally {
    fn valid(&self) -> bool {
        let kinds_valid = |kinds: &BTreeMap<String, u64>| kinds.len() <= KIND_CAP + 1 && kinds.keys().all(|key| is_kind(key));
        self.by_bundle.len() <= BUNDLE_CAP + 1 && kinds_valid(&self.refusals)
            && self.by_bundle.iter().all(|(bundle, counts)| is_bundle(bundle) && kinds_valid(&counts.refusals))
    }

    fn add(&mut self, input: &Value, receipt: &Receipt, refusal: Option<&str>, now_ms: u64) {
        if self.since == 0 { self.since = now_ms; }
        self.calls = self.calls.saturating_add(1);
        let bundle = if receipt.several_bundles { "unknown" } else { receipt.bundle.as_deref().unwrap_or("unknown") };
        let bundle = bucket(&self.by_bundle, bundle, BUNDLE_CAP);
        let counts = self.by_bundle.entry(bundle).or_default();
        counts.calls = counts.calls.saturating_add(1);
        if let Some(code) = refusal {
            increment(&mut self.refusals, code);
            increment(&mut counts.refusals, code);
            if code == ROUTE_REFUSAL { self.browser_route_refusals = self.browser_route_refusals.saturating_add(1); }
        }
        if declares_chrome(input) { self.browser_chrome_calls = self.browser_chrome_calls.saturating_add(1); }
        if receipt.unknown_classification && input.get("action").and_then(Value::as_str).map(str::trim).is_some_and(super::computer_tools::acts) {
            self.browser_classification_unknown = self.browser_classification_unknown.saturating_add(1);
        }
    }
}

fn declares_chrome(input: &Value) -> bool {
    input.get("browser_chrome").is_some_and(|value| value == &Value::Bool(true)
        || value.as_str().is_some_and(|text| matches!(text.trim().to_ascii_lowercase().as_str(), "true" | "1")))
        || input.get("steps").and_then(Value::as_array).is_some_and(|steps| steps.iter().any(declares_chrome))
}

fn inspect(answer: &Value, receipt: &mut Receipt, depth: usize) {
    if depth == 0 { return; }
    if let Some(bundle) = answer.pointer("/app/bundleId").and_then(Value::as_str).filter(|bundle| is_bundle(bundle)) {
        let bundle = bundle.to_ascii_lowercase();
        if receipt.bundle.as_ref().is_some_and(|previous| previous != &bundle) { receipt.several_bundles = true; }
        receipt.bundle.get_or_insert(bundle);
    }
    receipt.unknown_classification |= answer.get("browserClassificationUnknown") == Some(&Value::Bool(true));
    if answer.get("ok") == Some(&Value::Bool(false)) {
        receipt.refusal = answer.pointer("/error/code").and_then(Value::as_str).map(|code| kind(code).to_string());
    }
    for key in ["result", "snapshot", "report"] {
        if let Some(child) = answer.get(key) { inspect(child, receipt, depth - 1); }
    }
    if let Some(steps) = answer.get("steps").and_then(Value::as_array) {
        for step in steps { inspect(step, receipt, depth - 1); }
    }
}

/// Retain only native identity and classification facts before an error becomes plain tool text.
pub(crate) fn receipt(answer: &Value) {
    ACTIVE.with(|active| {
        if let Some(receipt) = active.borrow_mut().as_mut() { inspect(answer, receipt, 8); }
    });
}

fn state_root(ctx: &ToolContext) -> Option<(PathBuf, usize)> {
    #[cfg(test)]
    { let _ = ctx; tests::STATE_ROOT.with(|root| root.borrow().clone()).map(|root| (root, 1)) }
    #[cfg(not(test))]
    {
        let cwd = ctx.cwd.clone().or_else(|| std::env::current_dir().ok())?;
        Some((runtime::zo_project_state_dir(&cwd), 3))
    }
}

fn save(root: &Path, owned_suffix: usize, input: &Value, receipt: &Receipt, refusal: Option<&str>) {
    let _lock = WRITE_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = root.join(FILE);
    let mut tally = runtime::secure_fs::read_regular_file_absolute_no_follow_bounded(&path, FILE_CAP).ok().flatten()
        .and_then(|text| serde_json::from_str::<Tally>(&text).ok()).filter(Tally::valid).unwrap_or_default();
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).ok().and_then(|time| u64::try_from(time.as_millis()).ok()).unwrap_or_default();
    tally.add(input, receipt, refusal, now_ms);
    if let Ok(bytes) = serde_json::to_vec(&tally) {
        if runtime::secure_fs::ensure_private_dir_absolute(root, owned_suffix).is_ok() {
            let _ = runtime::secure_fs::write_atomic_owner_only(root, Path::new(FILE), &bytes);
        }
    }
}

/// Count once around the entire Computer call, including its preparation and final look.
pub(crate) fn run(
    input: &Value,
    ctx: &ToolContext,
    execute: impl FnOnce() -> Result<ToolExecutionResult, ToolError>,
) -> Result<ToolExecutionResult, ToolError> {
    let _collection = Collection(ACTIVE.with(|active| active.replace(Some(Receipt::default()))));
    let result = execute();
    let receipt = ACTIVE.with(|active| active.borrow().clone()).unwrap_or_default();
    let refusal = match &result {
        Err(ToolError::Execution(text)) => Some(kind(text.split_once(':').map_or("other", |(code, _)| code)).to_string()),
        Err(ToolError::InvalidInput(_)) => Some("invalid_argument".to_string()),
        Err(ToolError::PermissionDenied { .. }) => Some("permission_denied".to_string()),
        Err(_) => Some("other".to_string()),
        Ok(result) if serde_json::from_str::<Value>(&result.text).ok().is_some_and(|answer| answer.get("ok") == Some(&Value::Bool(false))) => Some(receipt.refusal.clone().unwrap_or_else(|| "other".to_string())),
        Ok(_) => None,
    };
    if let Some((root, owned_suffix)) = state_root(ctx) { save(&root, owned_suffix, input, &receipt, refusal.as_deref()); }
    result
}

#[cfg(test)]
mod tests;
