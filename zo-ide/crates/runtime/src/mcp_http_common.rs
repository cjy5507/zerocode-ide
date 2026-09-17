//! Shared HTTP utilities for MCP remote transports (SSE, HTTP, WebSocket).
//!
//! Provides `build_http_client`, `apply_headers`, and
//! `resolve_headers_helper` so that every transport module can share a
//! single, consistent implementation instead of duplicating code.

use std::collections::BTreeMap;
use std::io;
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use crate::mcp_stdio::InboundEvent;

/// A deduplicating, drainable buffer of inbound MCP events captured off a remote
/// transport's read path.
///
/// The SSE and Streamable-HTTP transports both record `tools/list_changed` (and
/// similar) notifications while *reading a response over `&self`*, so the buffer
/// needs interior mutability — hence the `std::sync::Mutex`. The critical section
/// is only a `Vec` push/take and is never held across an `.await`, so a blocking
/// mutex is correct and cheaper than the async one. (The stdio and WebSocket
/// transports drain their reads behind `&mut self` and so keep a plain, lock-free
/// `Vec` instead of this type.)
#[derive(Debug, Default)]
pub(crate) struct InboundBuffer {
    events: Mutex<Vec<InboundEvent>>,
}

impl InboundBuffer {
    /// Record `event`, skipping a duplicate already pending. Every variant is
    /// idempotent for the consumer — a refresh re-reads the server's *current*
    /// tool set — so one queued `ToolsListChanged` is indistinguishable from ten,
    /// and collapsing duplicates is both the bound and the correct semantics.
    pub(crate) fn push(&self, event: InboundEvent) {
        let mut events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        if !events.contains(&event) {
            events.push(event);
        }
    }

    /// Drain everything buffered since the last call; an empty `Vec` means
    /// nothing changed.
    pub(crate) fn drain(&self) -> Vec<InboundEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Construct a `reqwest::Client` with `headers` pre-loaded as default headers.
///
/// Building the client up-front means every request automatically carries the
/// configured headers without callers having to remember to add them.
pub(crate) fn build_http_client(headers: &BTreeMap<String, String>) -> io::Result<reqwest::Client> {
    let mut default_headers = reqwest::header::HeaderMap::new();
    for (key, value) in headers {
        let header_name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let header_value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        default_headers.insert(header_name, header_value);
    }

    reqwest::Client::builder()
        .default_headers(default_headers)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(io::Error::other)
}

/// Apply per-request headers from the transport config to a `RequestBuilder`.
///
/// This is a belt-and-suspenders companion to the default headers set in the
/// client: some middleware may strip or override default headers, so we also
/// set them explicitly on each request.
pub(crate) fn apply_headers(
    mut builder: reqwest::RequestBuilder,
    headers: &BTreeMap<String, String>,
) -> reqwest::RequestBuilder {
    for (key, value) in headers {
        builder = builder.header(key.as_str(), value.as_str());
    }
    builder
}

/// Apply per-request headers **and** inject a Bearer token for OAuth-configured servers.
///
/// If the server has a cached, non-expired OAuth token, this adds an
/// `Authorization: Bearer <token>` header automatically.
pub fn apply_headers_with_auth(
    builder: reqwest::RequestBuilder,
    headers: &BTreeMap<String, String>,
    server_name: Option<&str>,
) -> reqwest::RequestBuilder {
    let builder = apply_headers(builder, headers);
    if let Some(name) = server_name {
        if let Some(token) = crate::mcp_oauth::get_mcp_bearer_token(name) {
            return builder.header("Authorization", format!("Bearer {token}"));
        }
    }
    builder
}

/// Run the optional `headers_helper` script and parse its `KEY=VALUE` output
/// into a header map that can be merged with the statically configured headers.
///
/// The helper is invoked with no arguments.  Its stdout is expected to contain
/// one header per line in the form `Header-Name: value` or `HEADER_NAME=value`.
/// Lines that do not match either format are silently ignored.
///
/// If `headers_helper` is `None` or an empty string the function returns an
/// empty map without spawning any process.
///
/// # Security
///
/// When `project_scoped` is `true` the helper comes from a project-level config
/// file (for example `.zo/settings.json`). Because a malicious repo
/// could set `headers_helper` to an arbitrary command, execution is **refused**
/// for project-scoped helpers and a warning is logged to stderr.  Only
/// user-level (global) helpers are executed.
///
/// Errors from the helper process (non-zero exit, spawn failure, non-UTF-8
/// output) are surfaced as `io::Error` so the caller can decide whether to
/// propagate or treat them as best-effort.
pub(crate) fn resolve_headers_helper(
    headers_helper: Option<&str>,
    project_scoped: bool,
) -> io::Result<BTreeMap<String, String>> {
    let script = match headers_helper {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(BTreeMap::new()),
    };

    // Refuse to execute helpers originating from project-level config to
    // prevent command-injection via malicious repositories.
    if project_scoped {
        eprintln!(
            "warning: ignoring project-scoped headers_helper `{script}` — \
             only user-level (global) config may specify executable helpers"
        );
        return Ok(BTreeMap::new());
    }

    let output = Command::new(script).output().map_err(|e| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("headers_helper `{script}` failed to spawn: {e}"),
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "headers_helper `{script}` exited with status {}: {stderr}",
            output.status
        )));
    }

    let stdout = std::str::from_utf8(&output.stdout).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("headers_helper `{script}` produced non-UTF-8 output: {e}"),
        )
    })?;

    let mut extra = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Accept both "Header-Name: value" and "HEADER_NAME=value" formats.
        if let Some((key, value)) = line.split_once(": ") {
            extra.insert(key.trim().to_string(), value.trim().to_string());
        } else if let Some((key, value)) = line.split_once('=') {
            // Convert underscore-separated env-var style to header format.
            let header_key = key.trim().replace('_', "-");
            extra.insert(header_key, value.trim().to_string());
        }
    }

    Ok(extra)
}

/// Select the JSON-RPC response frame matching `expected_id` from an MCP
/// Streamable-HTTP / SSE response body.
///
/// The body is either a plain JSON object/array or a `text/event-stream`
/// carrying one or more `data:` events. Per the MCP spec a server may interleave
/// notifications (no `id` — e.g. `notifications/progress`, logging) or unrelated
/// responses *before* the one we asked for, so the frame must be selected by id
/// rather than taking the first `data:` line — the same id correlation the stdio
/// transport's read loop performs. A non-event-stream body is a single response
/// and is returned as-is (the caller's id check validates it).
///
/// Returns the matching frame's raw JSON text, or `None` if the event stream
/// carried no response with `expected_id`.
pub(crate) fn select_jsonrpc_response_frame<'a>(
    body: &'a str,
    expected_id: &crate::mcp_stdio::JsonRpcId,
) -> Option<&'a str> {
    let expected = serde_json::to_value(expected_id).ok()?;
    let trimmed = body.trim();

    // Plain JSON body (not an event stream): a single response object/array.
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Some(trimmed);
    }

    // SSE event stream: pick the `data:` frame whose JSON-RPC id matches,
    // skipping notifications (no `id`) and any other request's response.
    for line in trimmed.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if !data.starts_with('{') {
                continue;
            }
            let id_matches = serde_json::from_str::<serde_json::Value>(data)
                .ok()
                .and_then(|value| value.get("id").cloned())
                .is_some_and(|frame_id| frame_id == expected);
            if id_matches {
                return Some(data);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_http_client_with_no_headers() {
        let client = build_http_client(&BTreeMap::new());
        assert!(client.is_ok());
    }

    #[test]
    fn build_http_client_with_valid_headers() {
        let headers = BTreeMap::from([
            ("X-Api-Key".to_string(), "secret".to_string()),
            ("X-Version".to_string(), "1".to_string()),
        ]);
        let client = build_http_client(&headers);
        assert!(client.is_ok());
    }

    #[test]
    fn build_http_client_rejects_invalid_header_name() {
        // Header names must be ASCII and cannot contain control characters.
        let headers = BTreeMap::from([("\x00bad".to_string(), "value".to_string())]);
        let result = build_http_client(&headers);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn apply_headers_adds_all_entries() {
        let client = reqwest::Client::new();
        let builder = client.get("https://example.com");
        let headers = BTreeMap::from([
            ("X-One".to_string(), "1".to_string()),
            ("X-Two".to_string(), "2".to_string()),
        ]);
        // The builder is consumed; we just verify it does not panic and returns
        // a builder (the actual headers can only be verified by sending).
        let _builder = apply_headers(builder, &headers);
    }

    #[test]
    fn resolve_headers_helper_returns_empty_for_none() {
        let result = resolve_headers_helper(None, false).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn resolve_headers_helper_returns_empty_for_empty_string() {
        let result = resolve_headers_helper(Some(""), false).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn resolve_headers_helper_errors_on_missing_script() {
        let result = resolve_headers_helper(Some("/nonexistent/helper-script-xyz"), false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn resolve_headers_helper_rejects_project_scoped() {
        // Project-scoped helpers must be refused to prevent command injection
        // via malicious repository configs.
        let result = resolve_headers_helper(Some("/usr/bin/env"), true).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn select_frame_returns_plain_json_body_as_is() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        let id = crate::mcp_stdio::JsonRpcId::Number(1);
        assert_eq!(select_jsonrpc_response_frame(body, &id), Some(body));
    }

    #[test]
    fn select_frame_extracts_matching_sse_data_line() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{}}\n\n";
        let id = crate::mcp_stdio::JsonRpcId::Number(7);
        let frame = select_jsonrpc_response_frame(body, &id).expect("matching frame");
        assert!(frame.contains("\"id\":7"));
    }

    #[test]
    fn select_frame_skips_notifications_before_the_response() {
        // Regression: an MCP server may stream `notifications/progress` (no id)
        // before the actual response. Selecting the first `data:` frame would
        // grab the notification and fail; we must select by id.
        let body = "event: message\n\
             data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"progress\":1}}\n\n\
             event: message\n\
             data: {\"jsonrpc\":\"2.0\",\"id\":42,\"result\":{\"ok\":true}}\n\n";
        let id = crate::mcp_stdio::JsonRpcId::Number(42);
        let frame = select_jsonrpc_response_frame(body, &id).expect("response frame");
        assert!(frame.contains("\"id\":42"));
        assert!(frame.contains("\"ok\":true"));
        assert!(!frame.contains("notifications/progress"));
    }

    #[test]
    fn select_frame_skips_other_request_ids() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"wrong\":true}}\n\n\
             data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"right\":true}}\n\n";
        let id = crate::mcp_stdio::JsonRpcId::Number(2);
        let frame = select_jsonrpc_response_frame(body, &id).expect("id-2 frame");
        assert!(frame.contains("\"right\":true"));
    }

    #[test]
    fn select_frame_returns_none_when_no_id_matches() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n";
        let id = crate::mcp_stdio::JsonRpcId::Number(99);
        assert_eq!(select_jsonrpc_response_frame(body, &id), None);
    }

    #[test]
    fn inbound_buffer_dedups_and_drains() {
        let buffer = InboundBuffer::default();
        assert!(buffer.drain().is_empty(), "a fresh buffer drains empty");

        // A duplicate event collapses — one pending refresh is enough.
        buffer.push(InboundEvent::ToolsListChanged);
        buffer.push(InboundEvent::ToolsListChanged);
        assert_eq!(buffer.drain(), vec![InboundEvent::ToolsListChanged]);

        // Draining empties the buffer, so the next poll sees nothing.
        assert!(buffer.drain().is_empty(), "drain leaves the buffer empty");
    }
}
