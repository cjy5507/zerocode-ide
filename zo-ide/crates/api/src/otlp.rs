//! OTLP/HTTP (JSON) telemetry exporter — Claude Code monitoring parity.
//!
//! Claude Code exports usage metrics and events over OpenTelemetry; zo
//! honors the same environment contract so existing collector setups work
//! unchanged:
//!
//! - `ZO_ENABLE_TELEMETRY=1` (or the CC-compatible
//!   `CLAUDE_CODE_ENABLE_TELEMETRY=1`) — master switch, off by default.
//! - `OTEL_LOGS_EXPORTER=otlp` / `OTEL_METRICS_EXPORTER=otlp` — choose
//!   signals; anything else (console, none, unset) disables that signal.
//! - `OTEL_EXPORTER_OTLP_ENDPOINT` — default `http://localhost:4318`.
//! - `OTEL_EXPORTER_OTLP_HEADERS` — `key=value,key=value` (auth headers).
//! - `OTEL_METRIC_EXPORT_INTERVAL` / `OTEL_LOGS_EXPORT_INTERVAL` — ms,
//!   defaults 60000 / 5000 (CC defaults).
//! - `OTEL_SERVICE_NAME` — resource `service.name`, default `zo`.
//!
//! Transport is OTLP/HTTP with JSON encoding (`/v1/logs`, `/v1/metrics` on
//! the standard 4318 port). `OTEL_EXPORTER_OTLP_PROTOCOL=grpc` is not
//! supported — the exporter stays off and says so on stderr rather than
//! silently posting to a port that speaks a different protocol.
//!
//! Design: [`OtlpHttpSink::record`] only enqueues into a bounded channel —
//! telemetry must never stall the agent loop, so a full queue drops the
//! event. A dedicated worker thread owns the HTTP client (a single-threaded
//! tokio runtime, the same pattern `mcp_tools` uses) and flushes batches on
//! the configured intervals; metrics are cumulative monotonic sums
//! aggregated in the worker.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use core_types::hex::to_hex_lower;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use telemetry::{TelemetryEvent, TelemetrySink};

#[cfg(test)]
const MAX_OPEN_OTLP_SPANS: usize = 4;
#[cfg(not(test))]
const MAX_OPEN_OTLP_SPANS: usize = 1_024;

#[cfg(test)]
const OPEN_OTLP_SPAN_TTL: Duration = Duration::from_secs(60);
#[cfg(not(test))]
const OPEN_OTLP_SPAN_TTL: Duration = Duration::from_secs(60 * 60);

/// Resolved exporter configuration. `None` from [`OtlpExporterConfig::from_env`]
/// means "exporter off" — the zero-cost default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtlpExporterConfig {
    pub endpoint: String,
    pub headers: Vec<(String, String)>,
    pub logs_enabled: bool,
    pub metrics_enabled: bool,
    pub traces_enabled: bool,
    pub logs_interval: Duration,
    pub metrics_interval: Duration,
    pub traces_interval: Duration,
    pub service_name: String,
}

impl OtlpExporterConfig {
    /// Read the process environment. Returns `None` unless telemetry is
    /// explicitly enabled AND at least one signal selects the `otlp` exporter.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(&|key| std::env::var(key).ok())
    }

    /// Pure core for [`Self::from_env`] — testable without touching the
    /// process-global environment.
    #[must_use]
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let enabled = |key: &str| lookup(key).is_some_and(|v| v.trim() == "1");
        if !enabled("ZO_ENABLE_TELEMETRY") && !enabled("CLAUDE_CODE_ENABLE_TELEMETRY") {
            return None;
        }
        let wants_otlp =
            |key: &str| lookup(key).is_some_and(|v| v.trim().eq_ignore_ascii_case("otlp"));
        let logs_enabled = wants_otlp("OTEL_LOGS_EXPORTER");
        let metrics_enabled = wants_otlp("OTEL_METRICS_EXPORTER");
        // Traces are the CC "beta" signal (`OTEL_TRACES_EXPORTER`): one span
        // per HTTP request and per turn, grouped into a per-session trace.
        let traces_enabled = wants_otlp("OTEL_TRACES_EXPORTER");
        if !logs_enabled && !metrics_enabled && !traces_enabled {
            return None;
        }
        if let Some(protocol) = lookup("OTEL_EXPORTER_OTLP_PROTOCOL") {
            let protocol = protocol.trim().to_ascii_lowercase();
            if !protocol.is_empty() && !protocol.starts_with("http") {
                eprintln!(
                    "zo: OTEL_EXPORTER_OTLP_PROTOCOL={protocol} is not supported \
                     (only http/json); telemetry export disabled"
                );
                return None;
            }
        }
        let interval = |key: &str, default_ms: u64| {
            lookup(key)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|ms| *ms > 0)
                .map_or(Duration::from_millis(default_ms), Duration::from_millis)
        };
        Some(Self {
            endpoint: lookup("OTEL_EXPORTER_OTLP_ENDPOINT")
                .map(|v| v.trim().trim_end_matches('/').to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "http://localhost:4318".to_string()),
            headers: parse_otlp_headers(lookup("OTEL_EXPORTER_OTLP_HEADERS").as_deref()),
            logs_enabled,
            metrics_enabled,
            traces_enabled,
            logs_interval: interval("OTEL_LOGS_EXPORT_INTERVAL", 5_000),
            metrics_interval: interval("OTEL_METRIC_EXPORT_INTERVAL", 60_000),
            traces_interval: interval("OTEL_BSP_SCHEDULE_DELAY", 5_000),
            service_name: lookup("OTEL_SERVICE_NAME")
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "zo".to_string()),
        })
    }
}

/// `key=value,key=value` per the OTLP exporter spec; malformed pairs are
/// skipped rather than failing the whole exporter.
fn parse_otlp_headers(raw: Option<&str>) -> Vec<(String, String)> {
    raw.unwrap_or_default()
        .split(',')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let (key, value) = (key.trim(), value.trim());
            (!key.is_empty()).then(|| (key.to_string(), value.to_string()))
        })
        .collect()
}

/// Bounded-queue [`TelemetrySink`] that exports OTLP/HTTP JSON from a
/// dedicated worker thread.
pub struct OtlpHttpSink {
    tx: SyncSender<TelemetryEvent>,
}

impl OtlpHttpSink {
    /// Spawn the worker thread and return the sink handle. Dropping every
    /// clone of the handle disconnects the channel; the worker then performs
    /// a final flush and exits.
    #[must_use]
    pub fn spawn(config: OtlpExporterConfig) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel(4_096);
        // A failed spawn (resource exhaustion) leaves a sink whose sends all
        // drop — telemetry degrades, the agent does not.
        let _ = std::thread::Builder::new()
            .name("zo-otlp-export".to_string())
            .spawn(move || export_worker(&config, &rx));
        Self { tx }
    }
}

impl TelemetrySink for OtlpHttpSink {
    fn record(&self, event: TelemetryEvent) {
        // Never block the agent loop on telemetry: a full queue or a dead
        // worker just drops the event.
        match self.tx.try_send(event) {
            Ok(()) | Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {}
        }
    }
}

/// Process-global exporter, resolved from the environment exactly once: the
/// worker thread must be spawned once per process, not once per sub-agent or
/// per client rebuild.
static GLOBAL_SINK: std::sync::OnceLock<Option<std::sync::Arc<OtlpHttpSink>>> =
    std::sync::OnceLock::new();

/// The shared exporter sink, if telemetry export is enabled — `None` is the
/// zero-cost default.
#[must_use]
pub fn global_sink() -> Option<std::sync::Arc<dyn TelemetrySink>> {
    GLOBAL_SINK
        .get_or_init(|| {
            OtlpExporterConfig::from_env().map(|c| std::sync::Arc::new(OtlpHttpSink::spawn(c)))
        })
        .clone()
        .map(|sink| sink as std::sync::Arc<dyn TelemetrySink>)
}

/// Convenience for callers that just want a tracer wired to the global
/// exporter (the CLI attaches one to both the API client and the runtime).
#[must_use]
pub fn session_tracer_from_env(session_id: &str) -> Option<telemetry::SessionTracer> {
    global_sink().map(|sink| telemetry::SessionTracer::new(session_id, sink))
}

/// Counter key: metric name + flattened attribute pairs (sorted for a stable
/// identity).
type CounterKey = (&'static str, BTreeMap<String, String>);

fn export_worker(config: &OtlpExporterConfig, rx: &Receiver<TelemetryEvent>) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let client = crate::providers::shared_http_client();
    let started = SystemTime::now();
    let mut pending_logs: Vec<Value> = Vec::new();
    let mut counters: BTreeMap<CounterKey, u64> = BTreeMap::new();
    let mut spans = SpanAccumulator::default();
    // One session per process lifetime — parity with `claude_code.session.count`.
    *counters
        .entry(("zo_code.session.count", BTreeMap::new()))
        .or_insert(0) += 1;
    let mut next_logs_flush = Instant::now() + config.logs_interval;
    let mut next_metrics_flush = Instant::now() + config.metrics_interval;
    let mut next_traces_flush = Instant::now() + config.traces_interval;

    loop {
        let disconnected = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                accumulate_counters(&mut counters, &event);
                if config.logs_enabled {
                    pending_logs.push(event_to_log_record(&event));
                }
                if config.traces_enabled {
                    // Stamp arrival time: record() enqueues immediately, so the
                    // worker's recv time tracks the real event time closely.
                    spans.observe(&event, SystemTime::now());
                }
                false
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => true,
        };

        let now = Instant::now();
        if config.logs_enabled
            && !pending_logs.is_empty()
            && (disconnected || now >= next_logs_flush)
        {
            let body = logs_payload(&config.service_name, &std::mem::take(&mut pending_logs));
            post_json(&runtime, &client, config, "/v1/logs", &body);
            next_logs_flush = now + config.logs_interval;
        }
        if config.metrics_enabled && (disconnected || now >= next_metrics_flush) {
            let body = metrics_payload(&config.service_name, started, &counters);
            post_json(&runtime, &client, config, "/v1/metrics", &body);
            next_metrics_flush = now + config.metrics_interval;
        }
        if config.traces_enabled && (disconnected || now >= next_traces_flush) {
            let finished = spans.take_finished();
            if !finished.is_empty() {
                let body = traces_payload(&config.service_name, &finished);
                post_json(&runtime, &client, config, "/v1/traces", &body);
            }
            next_traces_flush = now + config.traces_interval;
        }
        if disconnected {
            return;
        }
    }
}

/// Builds OTLP spans by correlating start/end telemetry events. Each HTTP
/// request (`HttpRequestStarted` → `…Succeeded`/`…Failed`, keyed by
/// `session_id` + `attempt`) and each turn (`turn_started` → `turn_completed`/
/// `turn_failed`, keyed by `session_id` + `sequence`) becomes one span; all of
/// a session's spans share a `trace_id` derived from the session id.
#[derive(Default)]
struct SpanAccumulator {
    open_requests: BTreeMap<(String, u32), SystemTime>,
    open_turns: BTreeMap<(String, u64), SystemTime>,
    finished: Vec<Value>,
    counter: u64,
}

impl SpanAccumulator {
    fn observe(&mut self, event: &TelemetryEvent, at: SystemTime) {
        self.prune_open_spans(at);
        match event {
            TelemetryEvent::HttpRequestStarted {
                session_id,
                attempt,
                ..
            } => {
                self.open_requests
                    .insert((session_id.clone(), *attempt), at);
            }
            TelemetryEvent::HttpRequestSucceeded {
                session_id,
                attempt,
                path,
                status,
                ..
            } => {
                self.close_request(
                    session_id,
                    *attempt,
                    at,
                    "OK",
                    vec![
                        ("http.method", json!("POST")),
                        ("url.path", json!(path)),
                        ("http.status_code", json!(status)),
                    ],
                );
            }
            TelemetryEvent::HttpRequestFailed {
                session_id,
                attempt,
                path,
                error,
                retryable,
                ..
            } => {
                self.close_request(
                    session_id,
                    *attempt,
                    at,
                    "ERROR",
                    vec![
                        ("url.path", json!(path)),
                        ("error.message", json!(error)),
                        ("retryable", json!(retryable)),
                    ],
                );
            }
            TelemetryEvent::SessionTrace(record) if record.name == "turn_started" => {
                self.open_turns
                    .insert((record.session_id.clone(), record.sequence), at);
            }
            TelemetryEvent::SessionTrace(record)
                if record.name == "turn_completed" || record.name == "turn_failed" =>
            {
                let status = if record.name == "turn_failed" {
                    "ERROR"
                } else {
                    "OK"
                };
                let key = (record.session_id.clone(), record.sequence);
                let start = self.open_turns.remove(&key).unwrap_or(at);
                let attrs: Vec<(&str, Value)> = record
                    .attributes
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.clone()))
                    .collect();
                let span =
                    self.build_span(&record.session_id, &record.name, start, at, status, attrs);
                self.finished.push(span);
            }
            _ => {}
        }
        self.prune_open_spans(at);
    }

    fn prune_open_spans(&mut self, now: SystemTime) {
        retain_fresh_spans(&mut self.open_requests, now);
        retain_fresh_spans(&mut self.open_turns, now);
        prune_oldest_spans(&mut self.open_requests, MAX_OPEN_OTLP_SPANS);
        prune_oldest_spans(&mut self.open_turns, MAX_OPEN_OTLP_SPANS);
    }

    fn close_request(
        &mut self,
        session_id: &str,
        attempt: u32,
        end: SystemTime,
        status: &str,
        attrs: Vec<(&str, Value)>,
    ) {
        let start = self
            .open_requests
            .remove(&(session_id.to_string(), attempt))
            .unwrap_or(end);
        let mut attrs = attrs;
        attrs.push(("attempt", json!(attempt)));
        let span = self.build_span(session_id, "anthropic.messages", start, end, status, attrs);
        self.finished.push(span);
    }

    fn build_span(
        &mut self,
        session_id: &str,
        name: &str,
        start: SystemTime,
        end: SystemTime,
        status: &str,
        attrs: Vec<(&str, Value)>,
    ) -> Value {
        self.counter += 1;
        let trace_id = trace_id_hex(session_id);
        let span_id = span_id_hex(session_id, self.counter);
        let attributes: Vec<Value> = attrs
            .into_iter()
            .map(|(key, value)| json!({ "key": key, "value": any_value(&value) }))
            .collect();
        // OTLP status: 0=Unset, 1=Ok, 2=Error.
        let status_code = if status == "ERROR" { 2 } else { 1 };
        json!({
            "traceId": trace_id,
            "spanId": span_id,
            "name": name,
            "kind": 3, // SPAN_KIND_CLIENT
            "startTimeUnixNano": unix_nanos(start),
            "endTimeUnixNano": unix_nanos(end),
            "attributes": attributes,
            "status": { "code": status_code },
        })
    }

    fn take_finished(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.finished)
    }
}

fn retain_fresh_spans<K: Ord>(spans: &mut BTreeMap<K, SystemTime>, now: SystemTime) {
    spans.retain(|_, start| {
        now.duration_since(*start)
            .map_or(true, |age| age <= OPEN_OTLP_SPAN_TTL)
    });
}

fn prune_oldest_spans<K: Ord + Clone>(spans: &mut BTreeMap<K, SystemTime>, limit: usize) {
    while spans.len() > limit {
        let Some(oldest_key) = spans
            .iter()
            .min_by_key(|(_, started)| *started)
            .map(|(key, _)| key.clone())
        else {
            return;
        };
        spans.remove(&oldest_key);
    }
}

/// 16-byte trace id (hex) stable per session, so every span from one session
/// joins the same trace.
fn trace_id_hex(session_id: &str) -> String {
    let digest = Sha256::digest(session_id.as_bytes());
    to_hex_lower(&digest[..16])
}

/// 8-byte span id (hex), unique per span via the worker's monotonic counter.
fn span_id_hex(session_id: &str, counter: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update(b":");
    hasher.update(counter.to_be_bytes());
    let digest = hasher.finalize();
    to_hex_lower(&digest[..8])
}

fn traces_payload(service_name: &str, spans: &[Value]) -> Value {
    json!({
        "resourceSpans": [{
            "resource": resource_json(service_name),
            "scopeSpans": [{
                "scope": { "name": "zo" },
                "spans": spans,
            }],
        }]
    })
}

fn post_json(
    runtime: &tokio::runtime::Runtime,
    client: &reqwest::Client,
    config: &OtlpExporterConfig,
    path: &str,
    body: &Value,
) {
    let url = format!("{}{path}", config.endpoint);
    let mut request = client
        .post(url)
        .timeout(Duration::from_secs(10))
        .header("content-type", "application/json");
    for (key, value) in &config.headers {
        request = request.header(key.as_str(), value.as_str());
    }
    // Best effort: a down collector must never disturb the session.
    let _ = runtime.block_on(async { request.json(body).send().await });
}

/// Derive cumulative counters from the event stream. Token counts ride on
/// the runtime's `turn_completed` trace record; HTTP outcomes count API
/// requests the way `claude_code.api_request` / `api_error` do.
fn accumulate_counters(counters: &mut BTreeMap<CounterKey, u64>, event: &TelemetryEvent) {
    let mut bump = |name: &'static str, attrs: BTreeMap<String, String>, by: u64| {
        if by > 0 {
            *counters.entry((name, attrs)).or_insert(0) += by;
        }
    };
    match event {
        TelemetryEvent::HttpRequestSucceeded { .. } => bump(
            "zo_code.api_request",
            BTreeMap::from([("outcome".to_string(), "success".to_string())]),
            1,
        ),
        TelemetryEvent::HttpRequestFailed { .. } => bump(
            "zo_code.api_request",
            BTreeMap::from([("outcome".to_string(), "error".to_string())]),
            1,
        ),
        TelemetryEvent::SessionTrace(record) if record.name == "turn_completed" => {
            for (attr_key, metric_type) in [
                ("input_tokens", "input"),
                ("output_tokens", "output"),
                ("cache_read_input_tokens", "cacheRead"),
                ("cache_creation_input_tokens", "cacheCreation"),
            ] {
                let count = record.attributes.get(attr_key).and_then(Value::as_u64);
                if let Some(count) = count {
                    bump(
                        "zo_code.token.usage",
                        BTreeMap::from([("type".to_string(), metric_type.to_string())]),
                        count,
                    );
                }
            }
        }
        _ => {}
    }
}

fn unix_nanos(time: SystemTime) -> String {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

/// OTLP `AnyValue` JSON encoding for the attribute value types we emit.
/// Nested structures are stringified — collectors index scalars, not trees.
fn any_value(value: &Value) -> Value {
    match value {
        Value::String(s) => json!({ "stringValue": s }),
        Value::Bool(b) => json!({ "boolValue": b }),
        Value::Number(n) if n.is_i64() || n.is_u64() => {
            // proto3 JSON int64 is string-encoded.
            json!({ "intValue": n.to_string() })
        }
        Value::Number(n) => json!({ "doubleValue": n.as_f64().unwrap_or(0.0) }),
        other => json!({ "stringValue": other.to_string() }),
    }
}

fn attribute_list(attributes: &Map<String, Value>) -> Vec<Value> {
    attributes
        .iter()
        .map(|(key, value)| json!({ "key": key, "value": any_value(value) }))
        .collect()
}

const SEVERITY_INFO: u8 = 9;
const SEVERITY_ERROR: u8 = 17;

/// One telemetry event → one OTLP log record carrying an `event.name`
/// attribute (the `OTel` events convention CC follows).
fn event_to_log_record(event: &TelemetryEvent) -> Value {
    let (name, severity, mut attributes, timestamp) = match event {
        TelemetryEvent::HttpRequestStarted {
            session_id,
            attempt,
            method,
            path,
            attributes,
        } => (
            "zo_code.api_request_started".to_string(),
            SEVERITY_INFO,
            with_base_attrs(attributes, session_id, *attempt, method, path),
            None,
        ),
        TelemetryEvent::HttpRequestSucceeded {
            session_id,
            attempt,
            method,
            path,
            status,
            request_id,
            attributes,
        } => {
            let mut attrs = with_base_attrs(attributes, session_id, *attempt, method, path);
            attrs.insert("status".to_string(), json!(status));
            if let Some(request_id) = request_id {
                attrs.insert("request_id".to_string(), json!(request_id));
            }
            (
                "zo_code.api_request".to_string(),
                SEVERITY_INFO,
                attrs,
                None,
            )
        }
        TelemetryEvent::HttpRequestFailed {
            session_id,
            attempt,
            method,
            path,
            error,
            retryable,
            attributes,
        } => {
            let mut attrs = with_base_attrs(attributes, session_id, *attempt, method, path);
            attrs.insert("error".to_string(), json!(error));
            attrs.insert("retryable".to_string(), json!(retryable));
            (
                "zo_code.api_error".to_string(),
                SEVERITY_ERROR,
                attrs,
                None,
            )
        }
        TelemetryEvent::Analytics(event) => (
            format!("zo_code.{}.{}", event.namespace, event.action),
            SEVERITY_INFO,
            event.properties.clone(),
            None,
        ),
        TelemetryEvent::SessionTrace(record) => {
            let mut attrs = record.attributes.clone();
            attrs.insert("session.id".to_string(), json!(record.session_id));
            attrs.insert("sequence".to_string(), json!(record.sequence));
            (
                format!("zo_code.{}", record.name),
                SEVERITY_INFO,
                attrs,
                Some(u128::from(record.timestamp_ms) * 1_000_000),
            )
        }
    };
    attributes.insert("event.name".to_string(), json!(name));
    let time_unix_nano =
        timestamp.map_or_else(|| unix_nanos(SystemTime::now()), |nanos| nanos.to_string());
    json!({
        "timeUnixNano": time_unix_nano,
        "severityNumber": severity,
        "severityText": if severity >= SEVERITY_ERROR { "ERROR" } else { "INFO" },
        "body": { "stringValue": name },
        "attributes": attribute_list(&attributes),
    })
}

fn with_base_attrs(
    attributes: &Map<String, Value>,
    session_id: &str,
    attempt: u32,
    method: &str,
    path: &str,
) -> Map<String, Value> {
    let mut attrs = attributes.clone();
    attrs.insert("session.id".to_string(), json!(session_id));
    attrs.insert("attempt".to_string(), json!(attempt));
    attrs.insert("http.method".to_string(), json!(method));
    attrs.insert("url.path".to_string(), json!(path));
    attrs
}

fn resource_json(service_name: &str) -> Value {
    json!({
        "attributes": [
            { "key": "service.name", "value": { "stringValue": service_name } },
            { "key": "service.version", "value": { "stringValue": env!("CARGO_PKG_VERSION") } },
        ]
    })
}

fn logs_payload(service_name: &str, log_records: &[Value]) -> Value {
    json!({
        "resourceLogs": [{
            "resource": resource_json(service_name),
            "scopeLogs": [{
                "scope": { "name": "zo" },
                "logRecords": log_records,
            }],
        }]
    })
}

/// Cumulative monotonic sums — one metric per name, one data point per
/// distinct attribute set.
fn metrics_payload(
    service_name: &str,
    started: SystemTime,
    counters: &BTreeMap<CounterKey, u64>,
) -> Value {
    let start_nanos = unix_nanos(started);
    let now_nanos = unix_nanos(SystemTime::now());
    let mut metrics: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
    for ((name, attrs), sum) in counters {
        let attributes: Vec<Value> = attrs
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": { "stringValue": value } }))
            .collect();
        metrics.entry(name).or_default().push(json!({
            "startTimeUnixNano": start_nanos,
            "timeUnixNano": now_nanos,
            "asInt": sum.to_string(),
            "attributes": attributes,
        }));
    }
    let metrics: Vec<Value> = metrics
        .into_iter()
        .map(|(name, data_points)| {
            json!({
                "name": name,
                "unit": if name.contains("token") { "{token}" } else { "{count}" },
                "sum": {
                    "aggregationTemporality": 2,
                    "isMonotonic": true,
                    "dataPoints": data_points,
                },
            })
        })
        .collect();
    json!({
        "resourceMetrics": [{
            "resource": resource_json(service_name),
            "scopeMetrics": [{
                "scope": { "name": "zo" },
                "metrics": metrics,
            }],
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use telemetry::SessionTraceRecord;

    /// `UNIX_EPOCH + n` seconds — span timestamp fixtures.
    fn at(unix_secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(unix_secs)
    }

    fn lookup_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    /// 마스터 스위치 없이는 OTEL_* 가 전부 설정돼도 꺼져 있어야 한다.
    #[test]
    fn exporter_stays_off_without_explicit_enable() {
        let lookup = lookup_from(&[("OTEL_LOGS_EXPORTER", "otlp")]);
        assert_eq!(OtlpExporterConfig::from_lookup(&lookup), None);
        let lookup = lookup_from(&[("ZO_ENABLE_TELEMETRY", "1")]);
        assert_eq!(
            OtlpExporterConfig::from_lookup(&lookup),
            None,
            "enabled but no signal selects otlp"
        );
    }

    #[test]
    fn cc_compatible_env_contract_resolves_defaults_and_headers() {
        let lookup = lookup_from(&[
            ("CLAUDE_CODE_ENABLE_TELEMETRY", "1"),
            ("OTEL_METRICS_EXPORTER", "otlp"),
            (
                "OTEL_EXPORTER_OTLP_HEADERS",
                "Authorization=Bearer tok, X-Team =a",
            ),
            ("OTEL_METRIC_EXPORT_INTERVAL", "10000"),
        ]);
        let config = OtlpExporterConfig::from_lookup(&lookup).expect("enabled");
        assert_eq!(config.endpoint, "http://localhost:4318");
        assert!(config.metrics_enabled && !config.logs_enabled && !config.traces_enabled);
        assert_eq!(config.metrics_interval, Duration::from_millis(10_000));
        assert_eq!(config.logs_interval, Duration::from_millis(5_000));
        assert_eq!(
            config.headers,
            vec![
                ("Authorization".to_string(), "Bearer tok".to_string()),
                ("X-Team".to_string(), "a".to_string()),
            ]
        );
        assert_eq!(config.service_name, "zo");
    }

    /// grpc 프로토콜은 정직하게 거부(엉뚱한 포트에 JSON을 쏘지 않는다).
    #[test]
    fn grpc_protocol_disables_the_exporter() {
        let lookup = lookup_from(&[
            ("ZO_ENABLE_TELEMETRY", "1"),
            ("OTEL_LOGS_EXPORTER", "otlp"),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
        ]);
        assert_eq!(OtlpExporterConfig::from_lookup(&lookup), None);
    }

    #[test]
    fn turn_completed_trace_drives_token_usage_counters() {
        let mut counters = BTreeMap::new();
        let mut attributes = Map::new();
        attributes.insert("input_tokens".to_string(), json!(120));
        attributes.insert("output_tokens".to_string(), json!(30));
        attributes.insert("cache_read_input_tokens".to_string(), json!(900));
        let event = TelemetryEvent::SessionTrace(SessionTraceRecord {
            session_id: "s1".to_string(),
            sequence: 1,
            name: "turn_completed".to_string(),
            timestamp_ms: 1_700_000_000_000,
            attributes,
        });
        accumulate_counters(&mut counters, &event);
        accumulate_counters(&mut counters, &event);
        let usage_for = |metric_type: &str| {
            counters
                .get(&(
                    "zo_code.token.usage",
                    BTreeMap::from([("type".to_string(), metric_type.to_string())]),
                ))
                .copied()
        };
        assert_eq!(usage_for("input"), Some(240));
        assert_eq!(usage_for("output"), Some(60));
        assert_eq!(usage_for("cacheRead"), Some(1_800));
        assert_eq!(usage_for("cacheCreation"), None, "absent attr → no counter");
    }

    #[test]
    fn log_record_carries_event_name_and_proto3_int_encoding() {
        let event = TelemetryEvent::HttpRequestFailed {
            session_id: "s1".to_string(),
            attempt: 2,
            method: "POST".to_string(),
            path: "/v1/messages".to_string(),
            error: "boom".to_string(),
            retryable: true,
            attributes: Map::new(),
        };
        let record = event_to_log_record(&event);
        assert_eq!(record["severityNumber"], 17);
        assert_eq!(record["body"]["stringValue"], "zo_code.api_error");
        let attrs = record["attributes"].as_array().expect("attribute list");
        let attempt = attrs
            .iter()
            .find(|a| a["key"] == "attempt")
            .expect("attempt attr");
        assert_eq!(
            attempt["value"]["intValue"], "2",
            "proto3 JSON int64 must be string-encoded"
        );
        assert!(attrs.iter().any(|a| a["key"] == "event.name"));
    }

    #[test]
    fn metrics_payload_is_cumulative_monotonic_sum() {
        let counters = BTreeMap::from([(
            (
                "zo_code.token.usage",
                BTreeMap::from([("type".to_string(), "input".to_string())]),
            ),
            42u64,
        )]);
        let payload = metrics_payload("zo", SystemTime::now(), &counters);
        let metric = &payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0];
        assert_eq!(metric["name"], "zo_code.token.usage");
        assert_eq!(metric["sum"]["aggregationTemporality"], 2);
        assert_eq!(metric["sum"]["isMonotonic"], true);
        assert_eq!(metric["sum"]["dataPoints"][0]["asInt"], "42");
    }

    /// `OTEL_TRACES_EXPORTER=otlp` 단독으로도 익스포터가 켜진다.
    #[test]
    fn traces_signal_alone_enables_the_exporter() {
        let lookup = lookup_from(&[
            ("ZO_ENABLE_TELEMETRY", "1"),
            ("OTEL_TRACES_EXPORTER", "otlp"),
        ]);
        let config = OtlpExporterConfig::from_lookup(&lookup).expect("enabled");
        assert!(config.traces_enabled && !config.logs_enabled && !config.metrics_enabled);
    }

    /// HTTP 요청 start→succeeded 가 하나의 span 으로, 같은 세션은 같은 `trace_id`
    /// 를 공유하고, 실패는 ERROR(status code 2) span 이 된다.
    #[test]
    fn http_events_correlate_into_request_spans() {
        let mut spans = SpanAccumulator::default();
        let t0 = at(1_000);
        let started = TelemetryEvent::HttpRequestStarted {
            session_id: "s1".to_string(),
            attempt: 1,
            method: "POST".to_string(),
            path: "/v1/messages".to_string(),
            attributes: Map::new(),
        };
        let succeeded = TelemetryEvent::HttpRequestSucceeded {
            session_id: "s1".to_string(),
            attempt: 1,
            method: "POST".to_string(),
            path: "/v1/messages".to_string(),
            status: 200,
            request_id: None,
            attributes: Map::new(),
        };
        spans.observe(&started, t0);
        spans.observe(&succeeded, at(1_002));
        let finished = spans.take_finished();
        assert_eq!(finished.len(), 1);
        let span = &finished[0];
        assert_eq!(span["name"], "anthropic.messages");
        assert_eq!(span["status"]["code"], 1, "OK");
        assert_eq!(span["startTimeUnixNano"], "1000000000000");
        assert_eq!(span["endTimeUnixNano"], "1002000000000");
        let trace_id = span["traceId"].as_str().expect("trace id").to_string();
        assert_eq!(trace_id.len(), 32, "16-byte trace id in hex");

        // A failed request in the SAME session shares the trace id and is ERROR.
        let failed = TelemetryEvent::HttpRequestFailed {
            session_id: "s1".to_string(),
            attempt: 2,
            method: "POST".to_string(),
            path: "/v1/messages".to_string(),
            error: "boom".to_string(),
            retryable: true,
            attributes: Map::new(),
        };
        spans.observe(&failed, at(2_000));
        let finished = spans.take_finished();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0]["status"]["code"], 2, "ERROR");
        assert_eq!(
            finished[0]["traceId"], trace_id,
            "same session → same trace"
        );
        // Distinct spans within the trace get distinct span ids.
        assert_ne!(finished[0]["spanId"], span["spanId"]);
    }

    /// `turn_started`→`turn_completed` 가 토큰 attribute 를 단 turn span 이 된다.
    #[test]
    fn turn_records_correlate_into_turn_spans() {
        let mut spans = SpanAccumulator::default();
        let mut attrs = Map::new();
        attrs.insert("input_tokens".to_string(), json!(120));
        spans.observe(
            &TelemetryEvent::SessionTrace(SessionTraceRecord {
                session_id: "s1".to_string(),
                sequence: 3,
                name: "turn_started".to_string(),
                timestamp_ms: 0,
                attributes: Map::new(),
            }),
            at(5_000),
        );
        spans.observe(
            &TelemetryEvent::SessionTrace(SessionTraceRecord {
                session_id: "s1".to_string(),
                sequence: 3,
                name: "turn_completed".to_string(),
                timestamp_ms: 0,
                attributes: attrs,
            }),
            at(5_010),
        );
        let payload = traces_payload("zo", &spans.take_finished());
        let span = &payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(span["name"], "turn_completed");
        assert_eq!(span["startTimeUnixNano"], "5000000000000");
        assert_eq!(span["endTimeUnixNano"], "5010000000000");
        let has_tokens = span["attributes"]
            .as_array()
            .expect("attrs")
            .iter()
            .any(|a| a["key"] == "input_tokens");
        assert!(has_tokens, "turn span carries token attributes: {span}");
    }

    #[test]
    fn open_spans_are_bounded_and_expire() {
        let mut spans = SpanAccumulator::default();
        for attempt in 0..(u32::try_from(MAX_OPEN_OTLP_SPANS).expect("span cap fits u32") + 3) {
            spans.observe(
                &TelemetryEvent::HttpRequestStarted {
                    session_id: "s1".to_string(),
                    attempt,
                    method: "POST".to_string(),
                    path: "/v1/messages".to_string(),
                    attributes: Map::new(),
                },
                at(1_000),
            );
        }
        assert_eq!(spans.open_requests.len(), MAX_OPEN_OTLP_SPANS);

        spans.observe(
            &TelemetryEvent::HttpRequestStarted {
                session_id: "s2".to_string(),
                attempt: 1,
                method: "POST".to_string(),
                path: "/v1/messages".to_string(),
                attributes: Map::new(),
            },
            at(10_000),
        );
        assert_eq!(
            spans.open_requests.len(),
            1,
            "old unmatched spans should expire on later telemetry"
        );
    }

    #[test]
    fn prune_oldest_spans_removes_oldest_timestamps_without_sort_buffer() {
        let mut spans = BTreeMap::from([("newer", at(30)), ("oldest", at(10)), ("middle", at(20))]);

        prune_oldest_spans(&mut spans, 2);

        assert_eq!(spans.len(), 2);
        assert!(!spans.contains_key("oldest"));
        assert!(spans.contains_key("middle"));
        assert!(spans.contains_key("newer"));
    }
}
