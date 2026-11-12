//! SSE (Server-Sent Events) transport for MCP servers.
//!
//! Connects to an SSE endpoint via GET, discovers the message endpoint from the
//! initial `endpoint` event, then sends JSON-RPC requests via POST.  Mirrors the
//! async method interface of [`crate::mcp_stdio::McpStdioProcess`] so that the
//! server manager can treat both transports uniformly.

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::mcp_client::McpRemoteTransport;
use crate::mcp_http_common::{
    InboundBuffer, apply_headers_with_auth, build_http_client, read_bounded_body,
    resolve_headers_helper, select_jsonrpc_response_frame,
};
use crate::mcp_limits::guard_mcp_read_growth;
use crate::mcp_stdio::{
    InboundEvent, JsonRpcId, JsonRpcRequest, JsonRpcResponse, McpInitializeParams,
    McpInitializeResult, McpListResourcesParams, McpListResourcesResult, McpListToolsParams,
    McpListToolsResult, McpReadResourceParams, McpReadResourceResult, McpToolCallParams,
    McpToolCallResult, inbound_event_for_notification,
};

/// An active SSE connection to a remote MCP server.
///
/// The MCP SSE transport works as follows:
/// 1. GET the SSE URL to open a streaming connection.
/// 2. The server sends an `endpoint` event containing the POST URL for messages.
/// 3. All JSON-RPC requests are sent via POST to that message endpoint.
/// 4. Responses arrive either as SSE `message` events or as HTTP response bodies.
#[derive(Debug)]
pub struct McpSseProcess {
    /// Base URL of the SSE server (the original connection URL).
    base_url: String,
    /// POST endpoint for sending JSON-RPC messages, discovered from the SSE stream.
    message_endpoint: String,
    /// HTTP client used for all requests.
    client: reqwest::Client,
    /// Custom headers to include in every request.
    headers: BTreeMap<String, String>,
    /// The long-lived GET event stream. Classic SSE servers (2024-11-05)
    /// deliver responses and notifications here rather than in the POST reply,
    /// so the stream is kept open for the life of the connection and read by
    /// [`Self::read_stream_response`]. Behind a `Mutex` because requests take
    /// `&self` yet must mutate the underlying stream + parse buffer.
    stream: Arc<Mutex<SseStream>>,
    /// Inbound notifications (`tools/list_changed`) captured off the GET stream
    /// while reading responses, drained by [`Self::poll_inbound`]. Backed by a
    /// `std::sync` mutex (not the tokio stream mutex) so the synchronous manager
    /// poll can drain it without entering the async runtime.
    inbound: InboundBuffer,
    /// Server name keying this connection's OAuth bearer token, or `None` when the
    /// server is not OAuth-authenticated. Threaded into every request so the
    /// `Authorization: Bearer` header is (re)attached — picking up an externally
    /// refreshed token without reconnecting.
    auth_server_name: Option<String>,
}

/// The persistent SSE GET stream plus the bytes read but not yet consumed as a
/// complete event.
#[derive(Debug)]
struct SseStream {
    response: reqwest::Response,
    buffer: String,
    /// The head of a multi-byte character cut by the last chunk, kept until
    /// the next chunk finishes it ([`push_utf8_lossy`]).
    pending: Vec<u8>,
}

impl McpSseProcess {
    /// Connect to an SSE MCP server.
    ///
    /// Sends a GET request to the SSE URL, reads the initial `endpoint` event to
    /// discover the message POST URL, and returns a ready-to-use process handle.
    pub async fn connect(
        transport: &McpRemoteTransport,
        auth_server_name: Option<String>,
    ) -> io::Result<Self> {
        // Merge statically-configured headers with any extras from the helper script.
        let mut merged_headers = transport.headers.clone();
        let helper_headers = resolve_headers_helper(transport.headers_helper.as_deref(), false)?;
        merged_headers.extend(helper_headers);

        let client = build_http_client(&merged_headers)?;

        let mut request = client.get(&transport.url);
        request = apply_headers_with_auth(request, &merged_headers, auth_server_name.as_deref());
        request = request.header("Accept", "text/event-stream");

        let mut response = request
            .send()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;

        if !response.status().is_success() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!(
                    "SSE connection to {} failed with status {}",
                    transport.url,
                    response.status()
                ),
            ));
        }

        // Read the event stream incrementally and stop at the first `endpoint`
        // event. Reading the whole body with `.text()` would block until the
        // server closes the connection — but an SSE stream is long-lived by
        // design (it stays open for server→client messages), so a standard
        // server would only finish connecting after the request timeout fired.
        // The leftover `buffer` (bytes after the endpoint event) and the live
        // `response` are kept so later responses can be read off the stream.
        let mut buffer = String::new();
        let mut pending = Vec::new();
        let message_endpoint = 'discover: loop {
            while let Some(event) = take_next_event(&mut buffer) {
                match parse_endpoint_event(&event, &transport.url) {
                    Ok(endpoint) => break 'discover endpoint,
                    // Cross-origin endpoint is a security failure — surface it.
                    Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                        return Err(error);
                    }
                    // Any other event before the endpoint (comments, keep-alives)
                    // is ignored; keep reading.
                    Err(_) => {}
                }
            }
            match response
                .chunk()
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            {
                Some(chunk) => {
                    guard_mcp_read_growth(buffer.len(), chunk.len(), "SSE endpoint discovery")?;
                    push_utf8_lossy(&mut buffer, &mut pending, &chunk);
                }
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "SSE stream from {} ended before an endpoint event",
                            transport.url
                        ),
                    ));
                }
            }
        };

        Ok(Self {
            base_url: transport.url.clone(),
            message_endpoint,
            client,
            headers: merged_headers,
            stream: Arc::new(Mutex::new(SseStream {
                response,
                buffer,
                pending,
            })),
            inbound: InboundBuffer::default(),
            auth_server_name,
        })
    }

    /// Send a JSON-RPC request and read the response from the POST reply body.
    pub async fn request<TParams: Serialize, TResult: DeserializeOwned>(
        &self,
        id: JsonRpcId,
        method: impl Into<String>,
        params: Option<TParams>,
    ) -> io::Result<JsonRpcResponse<TResult>> {
        let method = method.into();
        let request = JsonRpcRequest::new(id.clone(), method.clone(), params);

        let body = serde_json::to_vec(&request)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let mut http_request = self.client.post(&self.message_endpoint);
        http_request = self.apply_auth(http_request);
        http_request = http_request.header("Content-Type", "application/json");

        let response = http_request
            .body(body)
            .send()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))?;

        if !response.status().is_success() {
            return Err(io::Error::other(format!(
                "SSE POST to {} for {method} failed with status {}",
                self.message_endpoint,
                response.status()
            )));
        }

        // A classic SSE server (2024-11-05) returns an empty 202 and delivers
        // the response as a `message` event on the GET stream. A Streamable-style
        // server may instead inline the response in the POST reply. Prefer an
        // inline response when present and valid; otherwise read the stream.
        let response_body = read_bounded_body(response).await?;

        if let Some(json_text) = select_jsonrpc_response_frame(&response_body, &id) {
            if let Ok(rpc_response) = serde_json::from_str::<JsonRpcResponse<TResult>>(json_text) {
                if rpc_response.jsonrpc == "2.0" && rpc_response.id == id {
                    return Ok(rpc_response);
                }
            }
        }

        self.read_stream_response(&method, &id).await
    }

    /// Read the JSON-RPC response with `expected_id` off the persistent SSE GET
    /// stream, skipping notifications (no `id`) and any other request's response
    /// — the same id correlation the stdio transport's read loop performs.
    async fn read_stream_response<TResult: DeserializeOwned>(
        &self,
        method: &str,
        expected_id: &JsonRpcId,
    ) -> io::Result<JsonRpcResponse<TResult>> {
        let expected = serde_json::to_value(expected_id)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut guard = self.stream.lock().await;
        let SseStream {
            response,
            buffer,
            pending,
        } = &mut *guard;
        loop {
            while let Some(event) = take_next_event(buffer) {
                let Some(data) = event_data_payload(&event) else {
                    continue;
                };
                let data = data.trim();
                if !data.starts_with('{') {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                // Correlate by id in a single lookup (mirrors the stdio read
                // loop): a notification has no `id`, another request's response
                // carries a non-matching one, and only our frame falls through.
                match value.get("id") {
                    // A notification (no `id`): surface `tools/list_changed` so a
                    // mid-session tool change is picked up, then keep reading for
                    // our response instead of dropping the frame.
                    None => {
                        if let Some(event) = inbound_event_for_notification(&value) {
                            self.inbound.push(event);
                        }
                        continue;
                    }
                    // Another request's response — unexpected with a single
                    // in-flight request, but keep waiting for ours.
                    Some(frame_id) if *frame_id != expected => continue,
                    Some(_) => {}
                }

                let rpc_response: JsonRpcResponse<TResult> = serde_json::from_value(value)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                if rpc_response.jsonrpc != "2.0" {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "SSE response for {method} used unsupported jsonrpc version `{}`",
                            rpc_response.jsonrpc
                        ),
                    ));
                }
                return Ok(rpc_response);
            }
            match response
                .chunk()
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            {
                Some(chunk) => {
                    guard_mcp_read_growth(buffer.len(), chunk.len(), "SSE event")?;
                    push_utf8_lossy(buffer, pending, &chunk);
                }
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "SSE stream from {} closed before a {method} response to {expected_id:?}",
                            self.base_url
                        ),
                    ));
                }
            }
        }
    }

    /// MCP `initialize` handshake.
    pub async fn initialize(
        &self,
        id: JsonRpcId,
        params: McpInitializeParams,
    ) -> io::Result<JsonRpcResponse<McpInitializeResult>> {
        self.request(id, "initialize", Some(params)).await
    }

    /// Send a JSON-RPC notification (no `id`, no response expected).
    pub async fn send_notification(&self, method: &str) -> io::Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method
        });
        let body = serde_json::to_vec(&notification)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let mut http_request = self.client.post(&self.message_endpoint);
        http_request = self.apply_auth(http_request);
        http_request = http_request.header("Content-Type", "application/json");

        let response = http_request
            .body(body)
            .send()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))?;

        if !response.status().is_success() {
            return Err(io::Error::other(format!(
                "SSE POST notification {method} failed with status {}",
                response.status()
            )));
        }
        Ok(())
    }

    /// MCP `tools/list`.
    pub async fn list_tools(
        &self,
        id: JsonRpcId,
        params: Option<McpListToolsParams>,
    ) -> io::Result<JsonRpcResponse<McpListToolsResult>> {
        self.request(id, "tools/list", params).await
    }

    /// MCP `tools/call`.
    pub async fn call_tool(
        &self,
        id: JsonRpcId,
        params: McpToolCallParams,
    ) -> io::Result<JsonRpcResponse<McpToolCallResult>> {
        self.request(id, "tools/call", Some(params)).await
    }

    /// MCP `resources/list`.
    pub async fn list_resources(
        &self,
        id: JsonRpcId,
        params: Option<McpListResourcesParams>,
    ) -> io::Result<JsonRpcResponse<McpListResourcesResult>> {
        self.request(id, "resources/list", params).await
    }

    /// MCP `resources/read`.
    pub async fn read_resource(
        &self,
        id: JsonRpcId,
        params: McpReadResourceParams,
    ) -> io::Result<JsonRpcResponse<McpReadResourceResult>> {
        self.request(id, "resources/read", Some(params)).await
    }

    /// Shut down the SSE connection.  SSE connections are stateless from the
    /// client's perspective so this is a no-op, but is provided for interface
    /// symmetry with `McpStdioProcess` and `McpHttpProcess`.
    pub fn shutdown(&self) -> io::Result<()> {
        Ok(())
    }

    /// Returns the discovered message endpoint URL.
    #[must_use]
    pub fn message_endpoint(&self) -> &str {
        &self.message_endpoint
    }

    /// Returns the base SSE URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Apply this connection's headers plus, for an OAuth-authenticated server,
    /// the current `Authorization: Bearer` token. The single point every POST
    /// routes through, so a request can never silently skip auth.
    fn apply_auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        apply_headers_with_auth(builder, &self.headers, self.auth_server_name.as_deref())
    }

    /// Drain inbound events captured off the GET stream since the last poll.
    pub fn poll_inbound(&mut self) -> Vec<InboundEvent> {
        self.inbound.drain()
    }
}

/// Append a network `chunk` to the text `buffer`, decoding UTF-8 across chunk
/// boundaries. A character of several bytes (a Hangul syllable is three) can be
/// cut between two chunks; decoding each chunk alone would turn both halves into
/// U+FFFD. The cut head waits in `pending` until the next chunk finishes it.
/// Bytes that are not UTF-8 at all still become U+FFFD, as
/// [`String::from_utf8_lossy`] makes them, so a bad byte never stops the stream.
fn push_utf8_lossy(buffer: &mut String, pending: &mut Vec<u8>, chunk: &[u8]) {
    pending.extend_from_slice(chunk);
    let mut consumed = 0;
    loop {
        let rest = &pending[consumed..];
        match std::str::from_utf8(rest) {
            Ok(text) => {
                buffer.push_str(text);
                consumed = pending.len();
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                buffer.push_str(&String::from_utf8_lossy(&rest[..valid]));
                consumed += valid;
                match error.error_len() {
                    Some(bad) => {
                        buffer.push(char::REPLACEMENT_CHARACTER);
                        consumed += bad;
                    }
                    // The bytes end inside a character: wait for the next chunk.
                    None => break,
                }
            }
        }
    }
    pending.drain(..consumed);
}

/// Pop the next complete SSE event (terminated by a blank line) from `buffer`,
/// consuming it through the terminator. Returns `None` while only a partial
/// event has been received. Accepts both `\n\n` and `\r\n\r\n` separators.
fn take_next_event(buffer: &mut String) -> Option<String> {
    let (idx, sep_len) = match (buffer.find("\n\n"), buffer.find("\r\n\r\n")) {
        (Some(lf), Some(crlf)) => {
            if lf <= crlf {
                (lf, 2)
            } else {
                (crlf, 4)
            }
        }
        (Some(lf), None) => (lf, 2),
        (None, Some(crlf)) => (crlf, 4),
        (None, None) => return None,
    };
    let event = buffer[..idx].to_string();
    buffer.drain(..idx + sep_len);
    Some(event)
}

/// Extract the `data:` payload from an SSE event block, joining multiple
/// `data:` lines with `\n` per the SSE spec. `None` if the event has no data.
fn event_data_payload(event: &str) -> Option<String> {
    let mut payload = String::new();
    let mut found = false;
    for line in event.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            if found {
                payload.push('\n');
            }
            payload.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            found = true;
        }
    }
    found.then_some(payload)
}

/// Parse the `endpoint` event from SSE stream body to discover the message URL.
///
/// The SSE stream should contain an event like:
/// ```text
/// event: endpoint
/// data: /message?sessionId=abc123
/// ```
fn parse_endpoint_event(body: &str, base_url: &str) -> io::Result<String> {
    let mut in_endpoint_event = false;

    for line in body.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("event:") {
            let event_type = trimmed.trim_start_matches("event:").trim();
            in_endpoint_event = event_type == "endpoint";
            continue;
        }

        if in_endpoint_event && trimmed.starts_with("data:") {
            let data = trimmed.trim_start_matches("data:").trim();
            return resolve_endpoint_url(base_url, data);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("SSE stream from {base_url} did not contain an endpoint event"),
    ))
}

/// Extract the origin (scheme + host + port) from a URL.
fn extract_origin(url: &str) -> Option<&str> {
    let scheme_end = url.find("://")?;
    let after_scheme = &url[scheme_end + 3..];
    let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
    Some(&url[..scheme_end + 3 + host_end])
}

/// Resolve a potentially relative endpoint path against the base SSE URL.
///
/// # Security
///
/// If the endpoint is an absolute URL, its origin (scheme + host + port) must
/// match the base SSE URL's origin.  Cross-origin redirects are rejected to
/// prevent SSRF attacks where a compromised MCP server redirects requests
/// (including auth headers) to an attacker-controlled host.
fn resolve_endpoint_url(base_url: &str, endpoint: &str) -> io::Result<String> {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        // Validate same-origin
        let base_origin = extract_origin(base_url).unwrap_or(base_url);
        let endpoint_origin = extract_origin(endpoint).unwrap_or(endpoint);
        if !base_origin.eq_ignore_ascii_case(endpoint_origin) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "SSE endpoint redirect to different origin is not allowed: \
                     base={base_origin}, endpoint={endpoint_origin}"
                ),
            ));
        }
        return Ok(endpoint.to_string());
    }

    // Extract the origin (scheme + host) from the base URL.
    if let Some(origin) = extract_origin(base_url) {
        if endpoint.starts_with('/') {
            return Ok(format!("{origin}{endpoint}"));
        }
        return Ok(format!("{origin}/{endpoint}"));
    }

    // Fallback: just concatenate.
    Ok(format!("{base_url}/{endpoint}"))
}

/// Connect to an MCP server over SSE transport.
///
/// This is the SSE counterpart to [`crate::mcp_stdio::spawn_mcp_stdio_process`].
/// `auth_server_name` is the OAuth token's storage key (`Some` only for an
/// OAuth-authenticated server), threaded into every request.
pub async fn connect_mcp_sse(
    transport: &McpRemoteTransport,
    auth_server_name: Option<String>,
) -> io::Result<McpSseProcess> {
    McpSseProcess::connect(transport, auth_server_name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_endpoint_event_from_sse_stream() {
        let body = "event: endpoint\ndata: /message?sessionId=abc123\n\n";
        let result = parse_endpoint_event(body, "https://mcp.example.com/sse").unwrap();
        assert_eq!(result, "https://mcp.example.com/message?sessionId=abc123");
    }

    #[test]
    fn parses_absolute_endpoint_url_same_origin() {
        let body = "event: endpoint\ndata: https://mcp.example.com/msg\n\n";
        let result = parse_endpoint_event(body, "https://mcp.example.com/sse").unwrap();
        assert_eq!(result, "https://mcp.example.com/msg");
    }

    #[test]
    fn rejects_cross_origin_endpoint_url() {
        let body = "event: endpoint\ndata: https://other.example.com/msg\n\n";
        let result = parse_endpoint_event(body, "https://mcp.example.com/sse");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn returns_error_when_no_endpoint_event() {
        let body = "event: message\ndata: hello\n\n";
        let result = parse_endpoint_event(body, "https://mcp.example.com/sse");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn resolves_relative_endpoint_against_base_url() {
        assert_eq!(
            resolve_endpoint_url("https://host.example.com/sse", "/message?s=1").unwrap(),
            "https://host.example.com/message?s=1"
        );
        assert_eq!(
            resolve_endpoint_url("https://host.example.com/sse", "message").unwrap(),
            "https://host.example.com/message"
        );
        assert_eq!(
            resolve_endpoint_url("https://host.example.com:8080/path/sse", "/msg").unwrap(),
            "https://host.example.com:8080/msg"
        );
    }

    #[test]
    fn rejects_cross_origin_endpoint_redirect() {
        let result = resolve_endpoint_url(
            "https://legit.example.com/sse",
            "https://attacker.example.com/steal",
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("different origin"));
    }

    #[test]
    fn allows_same_origin_absolute_endpoint() {
        assert_eq!(
            resolve_endpoint_url(
                "https://legit.example.com/sse",
                "https://legit.example.com/message?s=1"
            )
            .unwrap(),
            "https://legit.example.com/message?s=1"
        );
    }

    #[tokio::test]
    async fn connect_returns_endpoint_without_waiting_for_stream_close() {
        // Regression: an SSE stream is long-lived. `connect` must read the
        // `endpoint` event incrementally and return, not block on `.text()`
        // until the server closes the connection (or the request timeout fires).
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping SSE test: listener bind not permitted here");
                return;
            }
            Err(error) => panic!("bind: {error}"),
        };
        let addr = listener.local_addr().expect("addr");

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            // Drain the GET request headers.
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            // Send the endpoint event, then HOLD the connection open to emulate
            // a live server→client event stream.
            let response = "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/event-stream\r\n\r\n\
                 event: endpoint\n\
                 data: /message?s=1\n\n";
            stream.write_all(response.as_bytes()).await.expect("write");
            stream.flush().await.expect("flush");
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });

        let transport = McpRemoteTransport {
            url: format!("http://{addr}/sse"),
            headers: BTreeMap::new(),
            headers_helper: None,
            auth: crate::mcp_client::McpClientAuth::None,
        };

        let process = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            McpSseProcess::connect(&transport, None),
        )
        .await
        .expect("connect must not block on the open SSE stream")
        .expect("connect should succeed");
        assert!(process.message_endpoint.ends_with("/message?s=1"));

        server.abort();
    }

    #[test]
    fn take_next_event_splits_on_blank_line_and_keeps_partial() {
        let mut buf = "event: a\ndata: 1\n\nevent: b\ndata: 2".to_string();
        assert_eq!(
            take_next_event(&mut buf).as_deref(),
            Some("event: a\ndata: 1")
        );
        assert_eq!(buf, "event: b\ndata: 2");
        // Only a partial event remains.
        assert_eq!(take_next_event(&mut buf), None);
    }

    #[test]
    fn event_data_payload_extracts_and_joins_data_lines() {
        assert_eq!(
            event_data_payload("event: message\ndata: {\"a\":1}").as_deref(),
            Some("{\"a\":1}")
        );
        assert_eq!(
            event_data_payload("data: line1\ndata: line2").as_deref(),
            Some("line1\nline2")
        );
        assert_eq!(event_data_payload("event: ping\n: keep-alive"), None);
    }

    #[test]
    fn push_utf8_lossy_keeps_a_character_cut_between_chunks() {
        // Hangul is three bytes and the pictograph four: cut every which way,
        // the text must come out as it went in.
        let text = "data: 안녕 🙂 끝\n\n";
        let bytes = text.as_bytes();
        for first in 0..=bytes.len() {
            for second in first..=bytes.len() {
                let mut buffer = String::new();
                let mut pending = Vec::new();
                for chunk in [&bytes[..first], &bytes[first..second], &bytes[second..]] {
                    push_utf8_lossy(&mut buffer, &mut pending, chunk);
                }
                assert_eq!(buffer, text, "cut at {first} and {second}");
                assert!(pending.is_empty(), "cut at {first} and {second}");
            }
        }
    }

    #[test]
    fn push_utf8_lossy_turns_a_byte_that_is_no_utf8_into_one_replacement_char() {
        // A truly bad byte still becomes U+FFFD and the stream goes on, however
        // the chunks are cut: the result is what `from_utf8_lossy` gives the
        // whole input.
        let bytes: &[u8] = b"data: a\xFFb \xEC\x95\x88 c\xF0\x9F\n\n";
        let whole = String::from_utf8_lossy(bytes).into_owned();
        assert!(whole.contains('\u{FFFD}'));
        for cut in 0..=bytes.len() {
            let mut buffer = String::new();
            let mut pending = Vec::new();
            push_utf8_lossy(&mut buffer, &mut pending, &bytes[..cut]);
            push_utf8_lossy(&mut buffer, &mut pending, &bytes[cut..]);
            assert_eq!(buffer, whole, "cut at {cut}");
        }
    }

    #[tokio::test]
    async fn a_hangul_response_cut_between_network_chunks_comes_out_whole() {
        // The GET stream delivers the response event in three writes, cutting
        // a Hangul syllable and a pictograph in the middle. Decoding each chunk
        // alone would put U+FFFD into the tool's answer.
        use tokio::io::AsyncWriteExt;

        let answer = "안녕하세요 🙂 끝";
        let Some((addr, server)) =
            serve_sse_post_reply(move |mut get_stream, mut post_stream, id| async move {
                post_stream
                    .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .expect("write 202");
                post_stream.flush().await.expect("flush post");

                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "content": [{ "type": "text", "text": answer }], "isError": false }
                });
                let event = format!("event: message\ndata: {response}\n\n").into_bytes();
                let hangul = event
                    .windows(3)
                    .position(|window| window == "안".as_bytes())
                    .expect("the answer starts with a Hangul syllable");
                let pictograph = event
                    .windows(4)
                    .position(|window| window == "🙂".as_bytes())
                    .expect("the answer holds a pictograph");
                // One byte into the syllable, two bytes into the pictograph.
                let cuts = [0, hangul + 1, pictograph + 2, event.len()];
                for pair in cuts.windows(2) {
                    get_stream
                        .write_all(&event[pair[0]..pair[1]])
                        .await
                        .expect("write piece");
                    get_stream.flush().await.expect("flush piece");
                    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            })
            .await
        else {
            return;
        };
        let process = connect_to(addr).await;

        let call = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            process.call_tool(JsonRpcId::Number(7), echo_params()),
        )
        .await
        .expect("call must read the stream, not hang")
        .expect("call should succeed");

        assert_eq!(
            call.result.expect("call result").content[0].data["text"],
            serde_json::json!(answer),
            "a character cut between two chunks must not turn into U+FFFD"
        );

        server.abort();
    }

    /// A mock SSE server: it opens the GET stream with an `endpoint` event, then
    /// hands `on_post` the GET stream, the first POST connection and the
    /// request's id. The GET stream closes when `on_post` returns. `None` when
    /// this sandbox does not let a test listen.
    async fn serve_sse_post_reply<F, Fut>(
        on_post: F,
    ) -> Option<(std::net::SocketAddr, tokio::task::JoinHandle<()>)>
    where
        F: FnOnce(tokio::net::TcpStream, tokio::net::TcpStream, serde_json::Value) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping SSE test: listener bind not permitted here");
                return None;
            }
            Err(error) => panic!("bind: {error}"),
        };
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut get_stream, _) = listener.accept().await.expect("accept get");
            let mut buf = [0u8; 1024];
            let _ = get_stream.read(&mut buf).await;
            get_stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n\
                      event: endpoint\ndata: /message\n\n",
                )
                .await
                .expect("write endpoint");
            get_stream.flush().await.expect("flush get");

            let (mut post_stream, _) = listener.accept().await.expect("accept post");
            let mut pbuf = vec![0u8; 4096];
            let n = post_stream.read(&mut pbuf).await.expect("read post");
            let req = String::from_utf8_lossy(&pbuf[..n]);
            let body_start = req.find("\r\n\r\n").map_or(0, |i| i + 4);
            let request: serde_json::Value =
                serde_json::from_str(req[body_start..].trim()).expect("post json");
            on_post(get_stream, post_stream, request["id"].clone()).await;
        });
        Some((addr, server))
    }

    async fn connect_to(addr: std::net::SocketAddr) -> McpSseProcess {
        let transport = McpRemoteTransport {
            url: format!("http://{addr}/sse"),
            headers: BTreeMap::new(),
            headers_helper: None,
            auth: crate::mcp_client::McpClientAuth::None,
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            McpSseProcess::connect(&transport, None),
        )
        .await
        .expect("connect should not block")
        .expect("connect should succeed")
    }

    fn echo_params() -> McpToolCallParams {
        McpToolCallParams {
            name: "echo".to_string(),
            arguments: Some(serde_json::json!({})),
            meta: None,
        }
    }

    #[tokio::test]
    async fn a_post_reply_past_the_message_cap_is_rejected_like_the_http_transport_does() {
        // `response.text()` has no limit: a server could stream any body into
        // memory. The POST reply is read through the same bounded reader the
        // Streamable HTTP transport uses, so one past the cap is refused.
        use tokio::io::AsyncWriteExt;

        let Some((addr, server)) = serve_sse_post_reply(|_get, mut post_stream, _id| async move {
            let length = crate::mcp_limits::MAX_MCP_MESSAGE_BYTES + 1;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {length}\r\n\r\n"
            );
            post_stream.write_all(head.as_bytes()).await.expect("head");
            let piece = vec![b' '; 1024 * 1024];
            let mut sent = 0;
            while sent < length {
                let take = piece.len().min(length - sent);
                // The client hangs up once it has refused the body.
                if post_stream.write_all(&piece[..take]).await.is_err() {
                    break;
                }
                sent += take;
            }
        })
        .await
        else {
            return;
        };
        let process = connect_to(addr).await;

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            process.call_tool(JsonRpcId::Number(3), echo_params()),
        )
        .await
        .expect("a body past the cap must be refused, not read to the end and then wait on the stream")
        .expect_err("a body past the cap must be an error");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("HTTP response body")
                && error.to_string().contains("limit"),
            "the same error the HTTP transport gives: {error}"
        );

        server.abort();
    }

    #[tokio::test]
    async fn a_post_reply_inside_the_cap_still_carries_the_response() {
        // A Streamable-style server inlines the response in the POST reply: the
        // bounded read changes nothing for a body inside the cap.
        use tokio::io::AsyncWriteExt;

        let Some((addr, server)) = serve_sse_post_reply(|_get, mut post_stream, id| async move {
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{ "type": "text", "text": "inline" }], "isError": false }
            })
            .to_string();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            post_stream.write_all(head.as_bytes()).await.expect("head");
            post_stream.write_all(body.as_bytes()).await.expect("body");
            post_stream.flush().await.expect("flush");
        })
        .await
        else {
            return;
        };
        let process = connect_to(addr).await;

        let call = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            process.call_tool(JsonRpcId::Number(4), echo_params()),
        )
        .await
        .expect("an inline reply must answer without the stream")
        .expect("call should succeed");
        assert_eq!(
            call.result.expect("call result").content[0].data["text"],
            serde_json::json!("inline")
        );

        server.abort();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // one cohesive SSE end-to-end scenario: POST/202 + GET-stream correlate
    async fn request_reads_response_from_the_event_stream() {
        // Classic SSE (2024-11-05): the POST returns an empty 202 and the
        // response arrives as a `message` event on the GET stream, after an
        // interleaved notification. The transport must read the stream and
        // correlate by id.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping SSE test: listener bind not permitted here");
                return;
            }
            Err(error) => panic!("bind: {error}"),
        };
        let addr = listener.local_addr().expect("addr");

        let server = tokio::spawn(async move {
            // 1. The GET event stream.
            let (mut get_stream, _) = listener.accept().await.expect("accept get");
            let mut buf = [0u8; 1024];
            let _ = get_stream.read(&mut buf).await;
            get_stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
                      Content-Type: text/event-stream\r\n\r\n\
                      event: endpoint\ndata: /message\n\n",
                )
                .await
                .expect("write endpoint");
            get_stream.flush().await.expect("flush get");

            // 2. The POST message connection.
            let (mut post_stream, _) = listener.accept().await.expect("accept post");
            let mut pbuf = vec![0u8; 4096];
            let n = post_stream.read(&mut pbuf).await.expect("read post");
            let req = String::from_utf8_lossy(&pbuf[..n]);
            let body_start = req.find("\r\n\r\n").map_or(0, |i| i + 4);
            let request: serde_json::Value =
                serde_json::from_str(req[body_start..].trim()).expect("post json");
            let id = request["id"].clone();
            post_stream
                .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                .await
                .expect("write 202");
            post_stream.flush().await.expect("flush post");

            // 3. Deliver a notification THEN the response on the GET stream.
            get_stream
                .write_all(
                    b"event: message\n\
                      data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"progress\":1}}\n\n",
                )
                .await
                .expect("write notification");
            // A `tools/list_changed` the transport must surface (not just skip).
            get_stream
                .write_all(
                    b"event: message\n\
                      data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
                )
                .await
                .expect("write list_changed");
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": "streamed" }],
                    "isError": false
                }
            });
            get_stream
                .write_all(format!("event: message\ndata: {response}\n\n").as_bytes())
                .await
                .expect("write response");
            get_stream.flush().await.expect("flush response");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        });

        let transport = McpRemoteTransport {
            url: format!("http://{addr}/sse"),
            headers: BTreeMap::new(),
            headers_helper: None,
            auth: crate::mcp_client::McpClientAuth::None,
        };
        let mut process = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            McpSseProcess::connect(&transport, None),
        )
        .await
        .expect("connect should not block")
        .expect("connect should succeed");

        let call = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            process.call_tool(
                JsonRpcId::Number(5),
                McpToolCallParams {
                    name: "echo".to_string(),
                    arguments: Some(serde_json::json!({})),
                    meta: None,
                },
            ),
        )
        .await
        .expect("call must read the stream, not hang")
        .expect("call should succeed");

        assert_eq!(call.id, JsonRpcId::Number(5));
        assert_eq!(
            call.result.expect("call result").content[0].data["text"],
            serde_json::json!("streamed")
        );

        // The `tools/list_changed` interleaved on the GET stream was captured
        // while reading the response and is now drainable; the unknown
        // `notifications/progress` is skipped (not surfaced).
        assert_eq!(process.poll_inbound(), vec![InboundEvent::ToolsListChanged]);
        assert!(
            process.poll_inbound().is_empty(),
            "the buffer drains on poll"
        );

        server.abort();
    }
}
