use std::fmt::{Debug, Formatter};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

mod harness_attest;

pub use harness_attest::{
    ablation_set, ablation_spec_error, attest_ablated, attest_declined, attest_failed, attest_fired,
    harness_attest_snapshot, AblationSet, AblationSpecError, FeatureAttestation, FeatureHealth,
    HarnessAttestLedger, HarnessFeature, ABLATED_REASON, ABLATION_ENV,
};

pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_APP_NAME: &str = "claude-cli";
/// Overrides the presented Claude Code version outright.
pub const CLAUDE_CODE_VERSION_ENV: &str = "ZO_CLAUDE_CODE_VERSION";
/// Set truthy, the installed CLI is never asked (hermetic runs).
pub const EXTERNAL_PROBES_DISABLED_ENV: &str = "ZO_DISABLE_EXTERNAL_CREDENTIALS";
/// What the client presents when nothing has named a version yet. Not a claim:
/// the API answers a client it finds too old by naming the version it wants,
/// and that answer is learned ([`learn_claude_code_version`]) so the retry
/// presents it. No number is pinned in this crate — a pinned one read 2.1.119
/// on 2026-09-02 while the installed CLI was 2.1.258, and the API refused
/// `claude-fable-5-1` to it.
pub const UNKNOWN_CLAUDE_CODE_VERSION: &str = "0.0.0";
/// The file under the zo cache directory that remembers what the API asked for.
const CLAUDE_CODE_VERSION_CACHE: &str = "claude-code-version";

/// The Claude Code version this client presents to the Anthropic API.
///
/// Behind a Claude Code login the API gates new models on the client's
/// version, so the number has to be the truth of this machine, not a
/// constant: [`CLAUDE_CODE_VERSION_ENV`] first; otherwise the newer of the
/// installed CLI (the `claude` on PATH is a symlink into
/// `…/versions/<version>`, read without spawning; `claude --version` when it
/// is not) and the version the API last asked for (remembered in the cache);
/// [`UNKNOWN_CLAUDE_CODE_VERSION`] when nothing answers. Resolved once per
/// process and raised only by [`learn_claude_code_version`].
#[must_use]
pub fn claude_code_version() -> String {
    non_empty_env(CLAUDE_CODE_VERSION_ENV).unwrap_or_else(|| LEDGER.presented())
}

/// The API said the presented version is too old and named the one it wants:
/// present that from now on, and remember it across processes. `true` when
/// the presented version rose — the caller's signal that a retry is worth
/// making; a second rejection at the same number answers `false`.
pub fn learn_claude_code_version(required: &str) -> bool {
    LEDGER.learn(required, cache_dir().as_deref())
}

/// `… version 2.1.251 or newer is required …` → `2.1.251`: the version an
/// Anthropic 400 names when it finds the client too old.
#[must_use]
pub fn required_claude_code_version_in(text: &str) -> Option<String> {
    let (before, _) = text.split_once("or newer is required")?;
    before
        .split_whitespace()
        .rev()
        .find_map(|word| parse_claude_code_version(word.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.')))
}

/// `2.1.258 (Claude Code)` → `2.1.258`: the first dotted number in what the
/// CLI printed, and nothing else — the parenthetical is the CLI's own label.
#[must_use]
pub fn parse_claude_code_version(printed: &str) -> Option<String> {
    printed
        .split(|ch: char| ch.is_whitespace() || ch == '(' || ch == ')')
        .map(str::trim)
        .find(|word| version_tuple(word).is_some())
        .map(str::to_string)
}

fn version_tuple(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let tuple = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(tuple)
}

fn newer(candidate: &str, held: &str) -> bool {
    match (version_tuple(candidate), version_tuple(held)) {
        (Some(new), Some(old)) => new > old,
        (Some(_), None) => true,
        _ => false,
    }
}

/// The one process-wide answer, resolved lazily and raised by what the API says.
struct VersionLedger {
    presented: OnceLock<std::sync::RwLock<String>>,
}

static LEDGER: VersionLedger = VersionLedger::new();

impl VersionLedger {
    const fn new() -> Self {
        Self {
            presented: OnceLock::new(),
        }
    }

    fn cell(&self) -> &std::sync::RwLock<String> {
        self.presented
            .get_or_init(|| std::sync::RwLock::new(resolve_initial_version()))
    }

    fn presented(&self) -> String {
        self.cell().read().map_or_else(|_| UNKNOWN_CLAUDE_CODE_VERSION.to_string(), |held| held.clone())
    }

    fn learn(&self, required: &str, cache_dir: Option<&Path>) -> bool {
        let Some(required) = parse_claude_code_version(required) else {
            return false;
        };
        let Ok(mut held) = self.cell().write() else {
            return false;
        };
        if !newer(&required, &held) {
            return false;
        }
        held.clone_from(&required);
        drop(held);
        if let Some(dir) = cache_dir {
            let _ = std::fs::create_dir_all(dir)
                .and_then(|()| std::fs::write(dir.join(CLAUDE_CODE_VERSION_CACHE), &required));
        }
        true
    }
}

/// The newer of what is installed and what the API last asked for.
fn resolve_initial_version() -> String {
    let remembered = cache_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join(CLAUDE_CODE_VERSION_CACHE)).ok())
        .and_then(|held| parse_claude_code_version(&held));
    let installed = installed_claude_code_version();
    match (installed, remembered) {
        (Some(installed), Some(remembered)) => {
            if newer(&remembered, &installed) {
                remembered
            } else {
                installed
            }
        }
        (Some(one), None) | (None, Some(one)) => one,
        (None, None) => UNKNOWN_CLAUDE_CODE_VERSION.to_string(),
    }
}

/// zo's cache directory, by the same lookup the config roots use
/// (`ZO_CONFIG_HOME`, then `ZO_HOME`, then `~/.zo`).
fn cache_dir() -> Option<PathBuf> {
    non_empty_env("ZO_CONFIG_HOME")
        .or_else(|| non_empty_env("ZO_HOME"))
        .map(PathBuf::from)
        .or_else(|| non_empty_env("HOME").map(|home| PathBuf::from(home).join(".zo")))
        .map(|root| root.join("cache"))
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn external_probes_disabled() -> bool {
    non_empty_env(EXTERNAL_PROBES_DISABLED_ENV)
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// The installed CLI's version: read off the `claude` symlink on PATH when it
/// points into a versioned install, else asked of the CLI itself once.
fn installed_claude_code_version() -> Option<String> {
    if external_probes_disabled() {
        return None;
    }
    non_empty_env("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .find_map(|dir| version_from_symlink_at(&dir))
        .or_else(spawned_claude_code_version)
}

/// `<dir>/claude -> …/versions/2.1.258[/…]` → `2.1.258`. A path component
/// that is a version is the install's own label; nothing runs.
fn version_from_symlink_at(dir: &Path) -> Option<String> {
    let target = std::fs::read_link(dir.join("claude")).ok()?;
    target
        .components()
        .rev()
        .find_map(|component| parse_claude_code_version(&component.as_os_str().to_string_lossy()))
}

fn spawned_claude_code_version() -> Option<String> {
    let output = std::process::Command::new("claude")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_claude_code_version(&String::from_utf8_lossy(&output.stdout)))
        .flatten()
}

pub const DEFAULT_RUNTIME: &str = "rust";
pub const DEFAULT_AGENTIC_BETA: &str = "claude-code-20250219";
pub const DEFAULT_PROMPT_CACHING_SCOPE_BETA: &str = "prompt-caching-scope-2026-01-05";
/// Enables 1h `ttl` on `cache_control` breakpoints — caches survive the
/// 5-minute default window across longer parallel-agent runs.
pub const DEFAULT_EXTENDED_CACHE_TTL_BETA: &str = "extended-cache-ttl-2025-04-11";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientIdentity {
    pub app_name: String,
    pub app_version: String,
    pub runtime: String,
}

impl ClientIdentity {
    #[must_use]
    pub fn new(app_name: impl Into<String>, app_version: impl Into<String>) -> Self {
        Self {
            app_name: app_name.into(),
            app_version: app_version.into(),
            runtime: DEFAULT_RUNTIME.to_string(),
        }
    }

    #[must_use]
    pub fn with_runtime(mut self, runtime: impl Into<String>) -> Self {
        self.runtime = runtime.into();
        self
    }

    /// The Claude Code identity presents the version the ledger holds NOW —
    /// the installed CLI's, or the one the API asked for — so a request retried
    /// after a rejection carries the learned number without rebuilding the
    /// client. Any other identity presents what it was built with.
    #[must_use]
    pub fn user_agent(&self) -> String {
        let version = if self.app_name == DEFAULT_APP_NAME {
            claude_code_version()
        } else {
            self.app_version.clone()
        };
        format!("{}/{} (external, cli)", self.app_name, version)
    }
}

impl Default for ClientIdentity {
    fn default() -> Self {
        Self::new(DEFAULT_APP_NAME, claude_code_version())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicRequestProfile {
    pub anthropic_version: String,
    pub client_identity: ClientIdentity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub betas: Vec<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_body: Map<String, Value>,
}

impl AnthropicRequestProfile {
    #[must_use]
    pub fn new(client_identity: ClientIdentity) -> Self {
        Self {
            anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_string(),
            client_identity,
            betas: vec![
                "interleaved-thinking-2025-05-14".to_string(),
                DEFAULT_AGENTIC_BETA.to_string(),
                DEFAULT_PROMPT_CACHING_SCOPE_BETA.to_string(),
                DEFAULT_EXTENDED_CACHE_TTL_BETA.to_string(),
            ],
            extra_body: Map::new(),
        }
    }

    #[must_use]
    pub fn with_beta(mut self, beta: impl Into<String>) -> Self {
        let beta = beta.into();
        if !self.betas.contains(&beta) {
            self.betas.push(beta);
        }
        self
    }

    #[must_use]
    pub fn with_extra_body(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra_body.insert(key.into(), value);
        self
    }

    #[must_use]
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        let mut headers = Vec::with_capacity(if self.betas.is_empty() { 3 } else { 4 });
        headers.extend([
            (
                "anthropic-version".to_owned(),
                self.anthropic_version.clone(),
            ),
            ("user-agent".to_owned(), self.client_identity.user_agent()),
            ("x-app".to_owned(), "cli".to_owned()),
        ]);
        if !self.betas.is_empty() {
            headers.push(("anthropic-beta".to_owned(), self.betas.join(",")));
        }
        headers
    }

    pub fn render_json_body<T: Serialize>(&self, request: &T) -> Result<Value, serde_json::Error> {
        let mut body = serde_json::to_value(request)?;
        let object = body.as_object_mut().ok_or_else(|| {
            serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request body must serialize to a JSON object",
            ))
        })?;
        object.extend(
            self.extra_body
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        // The Anthropic OAuth (Claude Max) path requires the `system` field
        // to be an array whose FIRST text block is exactly the Claude Code
        // identity string. A plain string or any other first block is
        // rejected as a fingerprint mismatch (returned as a 429).
        if let Some(system_blocks) = object
            .get("system")
            .and_then(Value::as_str)
            .map(claude_code_system_blocks)
        {
            object.insert("system".to_owned(), Value::Array(system_blocks));
        }
        // Fill any cache slots the caller left open, up to Anthropic's hard limit
        // of 4 — NEVER exceeding it (the bug this guards: the typed runtime-bridge
        // path already marks system + a rolling message window, and blindly adding
        // these on top produced `400: A maximum of 4 blocks with cache_control may
        // be provided. Found 5.` on the second turn). The defaults, in priority
        // order, are the last system block + last tool (the largest stable prefix)
        // and the last `user` message's tail block (so a long conversation re-pays
        // only for the newest exchange). Already-cached blocks cost no budget.
        let mut budget =
            MAX_CACHE_BREAKPOINTS.saturating_sub(count_cache_control_breakpoints(object));
        attach_cache_control_to_last_block(object, "system", &mut budget);
        attach_cache_control_to_last_block(object, "tools", &mut budget);
        attach_cache_control_to_last_user_message(object, &mut budget);
        // betas are sent via anthropic-beta header, not in the request body.
        Ok(body)
    }
}

impl Default for AnthropicRequestProfile {
    fn default() -> Self {
        Self::new(ClientIdentity::default())
    }
}

/// Anthropic's hard ceiling on `cache_control` breakpoints per request. Exceeding
/// it is a fatal `400 invalid_request_error: A maximum of 4 blocks with
/// cache_control may be provided`.
const MAX_CACHE_BREAKPOINTS: usize = 4;

/// Count the `cache_control` breakpoints a request already carries across its
/// system blocks, tools, and message content.
///
/// Callers such as the typed runtime-bridge path place their own breakpoints
/// (system split + a rolling window of message blocks). The default breakpoints
/// added by [`AnthropicRequestProfile::render_json_body`] must fill only the
/// *remaining* slots — stacking them unconditionally is what produced
/// `400 ... A maximum of 4 blocks with cache_control may be provided. Found 5.`
/// on the second turn.
fn count_cache_control_breakpoints(object: &Map<String, Value>) -> usize {
    // Count a block's own `cache_control` PLUS any inside its nested `content`
    // array (one level). Anthropic counts the nested ones too — e.g. a
    // `cache_control` on a tool_result's inner content block — so a shallow count
    // would under-reserve and let the fill below push the request past 4.
    fn in_array(array: Option<&Value>) -> usize {
        array.and_then(Value::as_array).map_or(0, |blocks| {
            blocks
                .iter()
                .map(|b| {
                    let own = usize::from(b.get("cache_control").is_some());
                    let nested = b
                        .get("content")
                        .and_then(Value::as_array)
                        .map_or(0, |inner| {
                            inner
                                .iter()
                                .filter(|c| c.get("cache_control").is_some())
                                .count()
                        });
                    own + nested
                })
                .sum()
        })
    }
    let mut total = in_array(object.get("system")) + in_array(object.get("tools"));
    if let Some(messages) = object.get("messages").and_then(Value::as_array) {
        total += messages
            .iter()
            .map(|m| in_array(m.get("content")))
            .sum::<usize>();
    }
    total
}

/// Mark the last element of the array field `field` (system blocks or tools) as
/// a 1h ephemeral cache breakpoint, but only if it does not already have one and
/// the remaining budget (`budget`) allows it. Decrements `budget` when it adds a
/// new breakpoint; an already-cached block is left untouched and costs nothing.
///
/// Cache breakpoints are positional — placing one at the end of system+tools
/// means every subsequent turn re-uses the cached prefix for free (does not
/// count toward ITPM). With 1h TTL, parallel sub-agents spawned from the
/// same parent share the cache for the full session lifetime.
fn attach_cache_control_to_last_block(
    object: &mut Map<String, Value>,
    field: &str,
    budget: &mut usize,
) {
    if *budget == 0 {
        return;
    }
    let Some(array) = object.get_mut(field).and_then(Value::as_array_mut) else {
        return;
    };
    let Some(last) = array.last_mut().and_then(Value::as_object_mut) else {
        return;
    };
    if last.contains_key("cache_control") {
        return;
    }
    last.insert(
        "cache_control".to_owned(),
        serde_json::json!({ "type": "ephemeral", "ttl": "1h" }),
    );
    *budget -= 1;
}

/// Find the last `role=user` message and mark its tail content block as a
/// 1h ephemeral cache breakpoint.
///
/// Why "last user" and not "every user": Anthropic prompt caching only
/// reads the longest cached prefix; placing a breakpoint at the last
/// user message captures *all* prior turns in one cache entry. Marking
/// multiple breakpoints would consume the 4-slot budget without
/// improving hit rate.
///
/// Why `user` over `assistant`: the next request always appends a new
/// user message (or tool_result-bearing user message) after the assistant
/// response, so caching up to the prior `user` boundary is what makes the
/// upcoming turn cheap. Caching past an assistant boundary would still
/// require re-billing the assistant tokens we already sent back.
///
/// String-form `content` is upgraded to a one-element block array because
/// `cache_control` only attaches to content *blocks*, not raw strings.
/// `budget`-aware: marks the tail block only if it is not already cached and a
/// slot remains, decrementing `budget` when it adds a new breakpoint.
fn attach_cache_control_to_last_user_message(object: &mut Map<String, Value>, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for msg in messages.iter_mut().rev() {
        let Some(m) = msg.as_object_mut() else {
            continue;
        };
        if m.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let breakpoint = serde_json::json!({ "type": "ephemeral", "ttl": "1h" });
        match m.get_mut("content") {
            Some(Value::Array(blocks)) => {
                if let Some(last_block) = blocks.last_mut().and_then(Value::as_object_mut) {
                    if !last_block.contains_key("cache_control") {
                        last_block.insert("cache_control".to_owned(), breakpoint);
                        *budget -= 1;
                    }
                }
            }
            Some(content_value) if content_value.is_string() => {
                let text = match std::mem::replace(content_value, Value::Null) {
                    Value::String(s) => s,
                    _ => String::new(),
                };
                *content_value = Value::Array(vec![serde_json::json!({
                    "type": "text",
                    "text": text,
                    "cache_control": breakpoint,
                })]);
                *budget -= 1;
            }
            _ => {}
        }
        return;
    }
}

fn claude_code_system_blocks(system_str: &str) -> Vec<Value> {
    const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

    let mut blocks = Vec::with_capacity(2);
    blocks.push(serde_json::json!({
        "type": "text",
        "text": CLAUDE_CODE_IDENTITY,
    }));

    let remainder = system_str.strip_prefix(CLAUDE_CODE_IDENTITY).map_or_else(
        || system_str.to_owned(),
        |rest| rest.trim_start_matches('\n').to_owned(),
    );
    if !remainder.is_empty() {
        blocks.push(serde_json::json!({
            "type": "text",
            "text": remainder,
        }));
    }

    blocks
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsEvent {
    pub namespace: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub properties: Map<String, Value>,
}

impl AnalyticsEvent {
    #[must_use]
    pub fn new(namespace: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            action: action.into(),
            properties: Map::new(),
        }
    }

    #[must_use]
    pub fn with_property(mut self, key: impl Into<String>, value: Value) -> Self {
        self.properties.insert(key.into(), value);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTraceRecord {
    pub session_id: String,
    pub sequence: u64,
    pub name: String,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attributes: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TelemetryEvent {
    HttpRequestStarted {
        session_id: String,
        attempt: u32,
        method: String,
        path: String,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        attributes: Map<String, Value>,
    },
    HttpRequestSucceeded {
        session_id: String,
        attempt: u32,
        method: String,
        path: String,
        status: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        attributes: Map<String, Value>,
    },
    HttpRequestFailed {
        session_id: String,
        attempt: u32,
        method: String,
        path: String,
        error: String,
        retryable: bool,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        attributes: Map<String, Value>,
    },
    Analytics(AnalyticsEvent),
    SessionTrace(SessionTraceRecord),
}

pub trait TelemetrySink: Send + Sync {
    fn record(&self, event: TelemetryEvent);
}

#[derive(Default)]
pub struct MemoryTelemetrySink {
    events: Mutex<Vec<TelemetryEvent>>,
}

impl MemoryTelemetrySink {
    #[must_use]
    pub fn events(&self) -> Vec<TelemetryEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl TelemetrySink for MemoryTelemetrySink {
    fn record(&self, event: TelemetryEvent) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

pub struct JsonlTelemetrySink {
    path: PathBuf,
    file: Mutex<File>,
}

impl Debug for JsonlTelemetrySink {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlTelemetrySink")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl JsonlTelemetrySink {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TelemetrySink for JsonlTelemetrySink {
    fn record(&self, event: TelemetryEvent) {
        let Ok(line) = serde_json::to_string(&event) else {
            return;
        };
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

#[derive(Clone)]
pub struct SessionTracer {
    session_id: String,
    sequence: Arc<AtomicU64>,
    sink: Arc<dyn TelemetrySink>,
}

impl Debug for SessionTracer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionTracer")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl SessionTracer {
    #[must_use]
    pub fn new(session_id: impl Into<String>, sink: Arc<dyn TelemetrySink>) -> Self {
        Self {
            session_id: session_id.into(),
            sequence: Arc::new(AtomicU64::new(0)),
            sink,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn record(&self, name: impl Into<String>, attributes: Map<String, Value>) {
        let record = SessionTraceRecord {
            session_id: self.session_id.clone(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
            timestamp_ms: current_timestamp_ms(),
            attributes,
        };
        self.sink.record(TelemetryEvent::SessionTrace(record));
    }

    pub fn record_http_request_started(
        &self,
        attempt: u32,
        method: impl Into<String>,
        path: impl Into<String>,
        attributes: Map<String, Value>,
    ) {
        let method = method.into();
        let path = path.into();
        let trace_attributes = merge_trace_fields(&method, &path, attempt, &attributes);
        self.sink.record(TelemetryEvent::HttpRequestStarted {
            session_id: self.session_id.clone(),
            attempt,
            method,
            path,
            attributes,
        });
        self.record("http_request_started", trace_attributes);
    }

    pub fn record_http_request_succeeded(
        &self,
        attempt: u32,
        method: impl Into<String>,
        path: impl Into<String>,
        status: u16,
        request_id: Option<String>,
        attributes: Map<String, Value>,
    ) {
        let method = method.into();
        let path = path.into();
        let mut trace_attributes = merge_trace_fields(&method, &path, attempt, &attributes);
        trace_attributes.insert("status".to_owned(), Value::from(status));
        if let Some(request_id) = request_id.as_ref() {
            trace_attributes.insert("request_id".to_owned(), Value::String(request_id.clone()));
        }
        self.sink.record(TelemetryEvent::HttpRequestSucceeded {
            session_id: self.session_id.clone(),
            attempt,
            method,
            path,
            status,
            request_id,
            attributes,
        });
        self.record("http_request_succeeded", trace_attributes);
    }

    pub fn record_http_request_failed(
        &self,
        attempt: u32,
        method: impl Into<String>,
        path: impl Into<String>,
        error: impl Into<String>,
        retryable: bool,
        attributes: Map<String, Value>,
    ) {
        let method = method.into();
        let path = path.into();
        let error = error.into();
        let mut trace_attributes = merge_trace_fields(&method, &path, attempt, &attributes);
        trace_attributes.insert("error".to_owned(), Value::String(error.clone()));
        trace_attributes.insert("retryable".to_owned(), Value::Bool(retryable));
        self.sink.record(TelemetryEvent::HttpRequestFailed {
            session_id: self.session_id.clone(),
            attempt,
            method,
            path,
            error,
            retryable,
            attributes,
        });
        self.record("http_request_failed", trace_attributes);
    }

    pub fn record_analytics(&self, event: AnalyticsEvent) {
        let mut attributes = event.properties.clone();
        attributes.insert(
            "namespace".to_owned(),
            Value::String(event.namespace.clone()),
        );
        attributes.insert("action".to_owned(), Value::String(event.action.clone()));
        self.sink.record(TelemetryEvent::Analytics(event));
        self.record("analytics", attributes);
    }

    pub fn record_security_audit(
        &self,
        action: impl Into<String>,
        mut attributes: Map<String, Value>,
    ) {
        attributes.insert("category".to_owned(), Value::String("security".to_string()));
        attributes.insert("action".to_owned(), Value::String(action.into()));
        self.record("security_audit", attributes);
    }
}

fn merge_trace_fields(
    method: &str,
    path: &str,
    attempt: u32,
    attributes: &Map<String, Value>,
) -> Map<String, Value> {
    let mut attributes = attributes.clone();
    attributes.insert("method".to_owned(), Value::String(method.to_owned()));
    attributes.insert("path".to_owned(), Value::String(path.to_owned()));
    attributes.insert("attempt".to_owned(), Value::from(attempt));
    attributes
}

fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_profile_emits_headers_and_merges_body() {
        let profile = AnthropicRequestProfile::new(
            ClientIdentity::new("claude-code", "1.2.3").with_runtime("rust-cli"),
        )
        .with_beta("tools-2026-04-01")
        .with_extra_body("metadata", serde_json::json!({"source": "test"}));

        let headers = profile.header_pairs();
        let beta = headers
            .iter()
            .find(|(k, _)| k == "anthropic-beta")
            .expect("beta header");
        assert!(
            beta.1.contains("claude-code-20250219"),
            "agentic beta: {}",
            beta.1
        );
        assert!(
            beta.1.contains("tools-2026-04-01"),
            "custom beta: {}",
            beta.1
        );
        let ua = headers
            .iter()
            .find(|(k, _)| k == "user-agent")
            .expect("user-agent");
        assert!(
            ua.1.contains("claude-code/1.2.3"),
            "version in UA: {}",
            ua.1
        );
        assert!(
            headers.iter().any(|(k, _)| k == "anthropic-version"),
            "should have version header"
        );
        assert_eq!(headers.len(), 4, "should have 4 headers: {headers:?}");

        let body = profile
            .render_json_body(&serde_json::json!({"model": "claude-sonnet"}))
            .expect("body should serialize");
        assert_eq!(
            body["metadata"]["source"],
            Value::String("test".to_string())
        );
        // betas are sent via header only, not in body
        assert!(body.get("betas").is_none());
    }

    #[test]
    fn request_profile_rewrites_system_prompt_into_identity_prefixed_blocks() {
        let profile = AnthropicRequestProfile::default();

        let body = profile
            .render_json_body(&serde_json::json!({
                "model": "claude-sonnet",
                "system": "You are Claude Code, Anthropic's official CLI for Claude.\nFollow the repo rules."
            }))
            .expect("body should serialize");

        assert_eq!(
            body["system"],
            serde_json::json!([
                {
                    "type": "text",
                    "text": "You are Claude Code, Anthropic's official CLI for Claude."
                },
                {
                    "type": "text",
                    "text": "Follow the repo rules.",
                    "cache_control": { "type": "ephemeral", "ttl": "1h" }
                }
            ])
        );
    }

    // Mirrors what Anthropic counts: a block's own cache_control plus any inside
    // its nested `content` array (one level).
    fn count_cache_control(body: &Value) -> usize {
        fn in_array(array: Option<&Value>) -> usize {
            array.and_then(Value::as_array).map_or(0, |blocks| {
                blocks
                    .iter()
                    .map(|b| {
                        let own = usize::from(b.get("cache_control").is_some());
                        let nested =
                            b.get("content")
                                .and_then(Value::as_array)
                                .map_or(0, |inner| {
                                    inner
                                        .iter()
                                        .filter(|c| c.get("cache_control").is_some())
                                        .count()
                                });
                        own + nested
                    })
                    .sum()
            })
        }
        let mut total = in_array(body.get("system")) + in_array(body.get("tools"));
        if let Some(messages) = body.get("messages").and_then(Value::as_array) {
            total += messages
                .iter()
                .map(|m| in_array(m.get("content")))
                .sum::<usize>();
        }
        total
    }

    #[test]
    fn render_does_not_exceed_four_breakpoints_on_a_pre_marked_request() {
        // Regression for `400: A maximum of 4 blocks with cache_control may be
        // provided. Found 5.` — the typed runtime-bridge path already marks 2
        // system + 2 message blocks (the full budget). render_json_body must NOT
        // stack its own system/tools/user breakpoints on top.
        let profile = AnthropicRequestProfile::default();
        let cc = serde_json::json!({ "type": "ephemeral", "ttl": "1h" });
        let request = serde_json::json!({
            "model": "claude-sonnet",
            "system": [
                { "type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude." },
                { "type": "text", "text": "static", "cache_control": cc },
                { "type": "text", "text": "dynamic", "cache_control": cc }
            ],
            "tools": [ { "name": "bash", "input_schema": { "type": "object" } } ],
            "messages": [
                { "role": "user", "content": [ { "type": "text", "text": "u1", "cache_control": cc } ] },
                { "role": "assistant", "content": [ { "type": "text", "text": "a1", "cache_control": cc } ] },
                { "role": "user", "content": [ { "type": "tool_result", "tool_use_id": "t1", "content": "ok" } ] }
            ]
        });
        let body = profile.render_json_body(&request).expect("serialize");
        assert_eq!(
            count_cache_control(&body),
            4,
            "pre-marked 4-slot budget must be preserved, not exceeded: {body}"
        );
        // The tool gaining a breakpoint was the 5th — it must stay uncached here.
        assert!(
            body["tools"][0].get("cache_control").is_none(),
            "render must not add a tool breakpoint to a pre-marked request: {body}"
        );
    }

    #[test]
    fn render_applies_default_breakpoints_to_a_raw_request() {
        // A request with no breakpoints (e.g. a sub-agent) still gets the default
        // system + tools + last-user caching — three, within the 4-slot budget.
        let profile = AnthropicRequestProfile::default();
        let request = serde_json::json!({
            "model": "claude-sonnet",
            "system": [ { "type": "text", "text": "sys" } ],
            "tools": [ { "name": "bash", "input_schema": { "type": "object" } } ],
            "messages": [ { "role": "user", "content": [ { "type": "text", "text": "hi" } ] } ]
        });
        let body = profile.render_json_body(&request).expect("serialize");
        assert_eq!(
            count_cache_control(&body),
            3,
            "default breakpoints applied to a raw request: {body}"
        );
        assert!(
            body["tools"][0].get("cache_control").is_some(),
            "tool gets a default breakpoint when none were pre-marked: {body}"
        );
    }

    #[test]
    fn render_fills_open_slots_up_to_the_limit_for_a_partially_marked_request() {
        // The non-TTY path splits the system prompt (2 breakpoints) but does NOT
        // mark messages. render must fill the 2 open slots (tool + last-user) up
        // to 4 — not skip them (losing the valuable last-user conversation cache)
        // and not exceed 4.
        let profile = AnthropicRequestProfile::default();
        let cc = serde_json::json!({ "type": "ephemeral", "ttl": "1h" });
        let request = serde_json::json!({
            "model": "claude-sonnet",
            "system": [
                { "type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude." },
                { "type": "text", "text": "static", "cache_control": cc },
                { "type": "text", "text": "dynamic", "cache_control": cc }
            ],
            "tools": [ { "name": "bash", "input_schema": { "type": "object" } } ],
            "messages": [ { "role": "user", "content": [ { "type": "text", "text": "u1" } ] } ]
        });
        let body = profile.render_json_body(&request).expect("serialize");
        assert_eq!(
            count_cache_control(&body),
            4,
            "the two open slots are filled to the 4-slot limit: {body}"
        );
        assert!(
            body["tools"][0].get("cache_control").is_some(),
            "the open tool slot is cached: {body}"
        );
    }

    #[test]
    fn render_counts_nested_tool_result_breakpoints_and_stays_within_the_limit() {
        // Defensive: a cache_control nested inside a tool_result's content array
        // must count toward the budget. With a shallow count the fill would push
        // the true total to 5; the one-level recursion keeps it at 4.
        let profile = AnthropicRequestProfile::default();
        let cc = serde_json::json!({ "type": "ephemeral", "ttl": "1h" });
        let request = serde_json::json!({
            "model": "claude-sonnet",
            "system": [
                { "type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude." },
                { "type": "text", "text": "static", "cache_control": cc }
            ],
            "tools": [ { "name": "bash", "input_schema": { "type": "object" } } ],
            "messages": [
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": [
                        { "type": "text", "text": "a", "cache_control": cc },
                        { "type": "text", "text": "b", "cache_control": cc }
                    ] }
                ] }
            ]
        });
        let body = profile.render_json_body(&request).expect("serialize");
        assert!(
            count_cache_control(&body) <= MAX_CACHE_BREAKPOINTS,
            "nested breakpoints must be counted so the total never exceeds 4: {body}"
        );
    }

    #[test]
    fn session_tracer_records_structured_events_and_trace_sequence() {
        let sink = Arc::new(MemoryTelemetrySink::default());
        let tracer = SessionTracer::new("session-123", sink.clone());

        tracer.record_http_request_started(1, "POST", "/v1/messages", Map::new());
        tracer.record_analytics(
            AnalyticsEvent::new("cli", "prompt_sent")
                .with_property("model", Value::String("claude-opus".to_string())),
        );

        let events = sink.events();
        assert!(matches!(
            &events[0],
            TelemetryEvent::HttpRequestStarted {
                session_id,
                attempt: 1,
                method,
                path,
                ..
            } if session_id == "session-123" && method == "POST" && path == "/v1/messages"
        ));
        assert!(matches!(
            &events[1],
            TelemetryEvent::SessionTrace(SessionTraceRecord { sequence: 0, name, .. })
            if name == "http_request_started"
        ));
        assert!(matches!(&events[2], TelemetryEvent::Analytics(_)));
        assert!(matches!(
            &events[3],
            TelemetryEvent::SessionTrace(SessionTraceRecord { sequence: 1, name, .. })
            if name == "analytics"
        ));
    }

    #[test]
    fn session_tracer_records_security_audit_events() {
        let sink = Arc::new(MemoryTelemetrySink::default());
        let tracer = SessionTracer::new("session-123", sink.clone());

        tracer.record_security_audit(
            "tool_execution_started",
            Map::from_iter([("tool_name".to_string(), Value::String("Bash".to_string()))]),
        );

        let events = sink.events();
        assert!(matches!(
            &events[0],
            TelemetryEvent::SessionTrace(SessionTraceRecord { name, attributes, .. })
            if name == "security_audit"
                && attributes.get("category").and_then(Value::as_str) == Some("security")
                && attributes.get("action").and_then(Value::as_str)
                    == Some("tool_execution_started")
                && attributes.get("tool_name").and_then(Value::as_str) == Some("Bash")
        ));
    }

    #[test]
    fn jsonl_sink_persists_events() {
        let path =
            std::env::temp_dir().join(format!("telemetry-jsonl-{}.log", current_timestamp_ms()));
        let sink = JsonlTelemetrySink::new(&path).expect("sink should create file");

        sink.record(TelemetryEvent::Analytics(
            AnalyticsEvent::new("cli", "turn_completed").with_property("ok", Value::Bool(true)),
        ));

        let contents = std::fs::read_to_string(&path).expect("telemetry log should be readable");
        assert!(contents.contains("\"type\":\"analytics\""));
        assert!(contents.contains("\"action\":\"turn_completed\""));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_installed_cli_version_is_read_from_what_it_prints() {
        assert_eq!(parse_claude_code_version("2.1.258 (Claude Code)").as_deref(), Some("2.1.258"));
        assert_eq!(parse_claude_code_version("  10.0.1\n").as_deref(), Some("10.0.1"));
        assert_eq!(parse_claude_code_version("Claude Code"), None);
        assert_eq!(parse_claude_code_version("v2.1"), None);
        assert_eq!(parse_claude_code_version("2.1.258.1"), None, "four parts is not a version");
    }

    #[test]
    fn the_version_the_api_asks_for_is_read_off_its_rejection() {
        let said = "Claude Code 2.1.119 does not support this model; version 2.1.251 or newer is \
                    required. Run 'claude update', or update the Claude desktop app, then try again.";
        assert_eq!(required_claude_code_version_in(said).as_deref(), Some("2.1.251"));
        assert_eq!(required_claude_code_version_in("invalid request: bad model"), None);
        assert_eq!(required_claude_code_version_in("version or newer is required"), None);
    }

    #[test]
    fn the_symlinked_install_names_its_version_without_running_anything() {
        let dir = std::env::temp_dir().join(format!("zo-claude-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("versions/9.9.9")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("versions/9.9.9"), dir.join("claude")).unwrap();
        #[cfg(unix)]
        assert_eq!(version_from_symlink_at(&dir).as_deref(), Some("9.9.9"));
        assert_eq!(version_from_symlink_at(&dir.join("versions")), None, "no claude here");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_ledger_rises_only_to_a_newer_version_and_remembers_it() {
        let ledger = VersionLedger::new();
        let _ = ledger.presented.set(std::sync::RwLock::new("2.1.100".to_string()));
        let cache = std::env::temp_dir().join(format!("zo-claude-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        assert!(ledger.learn("2.1.251", Some(&cache)));
        assert_eq!(ledger.presented(), "2.1.251");
        assert_eq!(
            std::fs::read_to_string(cache.join(CLAUDE_CODE_VERSION_CACHE)).unwrap(),
            "2.1.251"
        );
        assert!(!ledger.learn("2.1.200", Some(&cache)), "older is not learned");
        assert!(!ledger.learn("2.1.251", Some(&cache)), "the same number is not news");
        assert!(!ledger.learn("garbage", Some(&cache)));
        assert_eq!(ledger.presented(), "2.1.251");
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn the_default_identity_presents_a_dotted_version() {
        let identity = ClientIdentity::default();
        assert_eq!(identity.app_name, DEFAULT_APP_NAME);
        assert!(
            parse_claude_code_version(&identity.app_version).is_some(),
            "not a version: {}",
            identity.app_version
        );
        assert!(identity.user_agent().starts_with("claude-cli/"));
        // Another identity keeps the version it was built with.
        assert_eq!(
            ClientIdentity::new("claude-code", "9.9.9").user_agent(),
            "claude-code/9.9.9 (external, cli)"
        );
    }
}
