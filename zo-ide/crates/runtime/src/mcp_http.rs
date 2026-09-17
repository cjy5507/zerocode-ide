//! Streamable HTTP transport for MCP servers.
//!
//! Implements JSON-RPC over HTTP POST with optional session management via the
//! `Mcp-Session-Id` header.  Mirrors the async method interface of
//! [`crate::mcp_stdio::McpStdioProcess`] so that the server manager can treat
//! both transports uniformly.

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::mcp_client::McpRemoteTransport;
use crate::mcp_http_common::{
    apply_headers_with_auth, build_http_client, resolve_headers_helper,
    select_jsonrpc_response_frame, InboundBuffer,
};
use crate::mcp_limits::MAX_MCP_MESSAGE_BYTES;
use crate::mcp_stdio::{
    inbound_event_for_notification, InboundEvent, JsonRpcId, JsonRpcRequest, JsonRpcResponse,
    McpInitializeParams, McpInitializeResult, McpListResourcesParams, McpListResourcesResult,
    McpListToolsParams, McpListToolsResult, McpReadResourceParams, McpReadResourceResult,
    McpToolCallParams, McpToolCallResult,
};

/// Header name used by the MCP Streamable HTTP transport for session tracking.
const MCP_SESSION_ID_HEADER: &str = "Mcp-Session-Id";

/// An active HTTP connection to a remote MCP server using the Streamable HTTP
/// transport.
///
/// All JSON-RPC requests are sent as HTTP POST to a single endpoint.  The
/// server may return a `Mcp-Session-Id` header which must be echoed back in
/// subsequent requests.
#[derive(Debug)]
pub struct McpHttpProcess {
    /// The endpoint URL for JSON-RPC POST requests.
    url: String,
    /// HTTP client.
    client: reqwest::Client,
    /// Custom headers from the transport config.
    headers: BTreeMap<String, String>,
    /// Session ID returned by the server, echoed in subsequent requests.
    session_id: Arc<Mutex<Option<String>>>,
    /// Inbound notifications (`tools/list_changed`) captured from the SSE frames
    /// of a POST reply, drained by [`Self::poll_inbound`]. Streamable HTTP has no
    /// persistent server→client channel, so only notifications interleaved in a
    /// response body are seen — best-effort, but enough for the common case where
    /// a server emits `tools/list_changed` alongside a response.
    inbound: InboundBuffer,
    /// Server name keying this connection's OAuth bearer token, or `None` when the
    /// server is not OAuth-authenticated. Threaded into every request so the
    /// `Authorization: Bearer` header is (re)attached — picking up an externally
    /// refreshed token without reconnecting.
    auth_server_name: Option<String>,
}

impl McpHttpProcess {
    /// Create a new HTTP transport connection.
    ///
    /// Unlike SSE, no initial handshake is needed — the first JSON-RPC request
    /// (typically `initialize`) establishes the session.
    pub fn connect(
        transport: &McpRemoteTransport,
        auth_server_name: Option<String>,
    ) -> io::Result<Self> {
        // Merge statically-configured headers with any extras from the helper script.
        let mut merged_headers = transport.headers.clone();
        let helper_headers = resolve_headers_helper(transport.headers_helper.as_deref(), false)?;
        merged_headers.extend(helper_headers);

        let client = build_http_client(&merged_headers)?;

        Ok(Self {
            url: transport.url.clone(),
            client,
            headers: merged_headers,
            session_id: Arc::new(Mutex::new(None)),
            inbound: InboundBuffer::default(),
            auth_server_name,
        })
    }

    /// Send a JSON-RPC request and read the response.
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

        let mut http_request = self.client.post(&self.url);
        http_request = self.apply_auth(http_request);
        http_request = http_request
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        // Include session ID if we have one from a previous response.
        if let Some(session_id) = self.current_session_id() {
            http_request = http_request.header(MCP_SESSION_ID_HEADER, session_id);
        }

        let response = http_request
            .body(body)
            .send()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))?;

        if !response.status().is_success() {
            return Err(io::Error::other(format!(
                "HTTP POST to {} for {method} failed with status {}",
                self.url,
                response.status()
            )));
        }

        // Capture session ID from response headers.
        if let Some(value) = response.headers().get(MCP_SESSION_ID_HEADER) {
            if let Ok(session_id) = value.to_str() {
                self.set_session_id(session_id);
            }
        }

        let response_body = read_bounded_body(response).await?;

        // Capture any `tools/list_changed` interleaved in the response stream so a
        // mid-session tool change is surfaced (the manager re-discovers on poll).
        self.capture_inbound_notifications(&response_body);

        // The response may be plain JSON or an SSE event stream that interleaves
        // notifications (e.g. `notifications/progress`) before the response;
        // select the frame whose JSON-RPC id matches our request.
        let json_text = select_jsonrpc_response_frame(&response_body, &id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("no JSON-RPC response with id {id:?} in {method} response stream"),
            )
        })?;

        let rpc_response: JsonRpcResponse<TResult> = serde_json::from_str(json_text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        if rpc_response.jsonrpc != "2.0" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "HTTP response for {method} used unsupported jsonrpc version `{}`",
                    rpc_response.jsonrpc
                ),
            ));
        }

        if rpc_response.id != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "HTTP response for {method} used mismatched id: expected {id:?}, got {:?}",
                    rpc_response.id
                ),
            ));
        }

        Ok(rpc_response)
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

        let mut http_request = self.client.post(&self.url);
        http_request = self.apply_auth(http_request);
        http_request = http_request
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        if let Some(session_id) = self.current_session_id() {
            http_request = http_request.header(MCP_SESSION_ID_HEADER, session_id);
        }

        let response = http_request
            .body(body)
            .send()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))?;

        if !response.status().is_success() {
            return Err(io::Error::other(format!(
                "HTTP POST notification {method} failed with status {}",
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

    /// Shut down the HTTP session.
    ///
    /// Sends a DELETE request to terminate the server-side session if a session
    /// ID is present.  Failures are silently ignored since the server may not
    /// support explicit session termination.
    pub async fn shutdown(&self) -> io::Result<()> {
        if let Some(session_id) = self.current_session_id() {
            let mut request = self.client.delete(&self.url);
            request = self.apply_auth(request);
            request = request.header(MCP_SESSION_ID_HEADER, session_id);

            // Best-effort: ignore errors during shutdown.
            let _ = request.send().await;
        }
        Ok(())
    }

    /// Returns the current session ID, if any.
    #[must_use]
    pub fn current_session_id(&self) -> Option<String> {
        self.session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Returns the endpoint URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Apply this connection's headers plus, for an OAuth-authenticated server,
    /// the current `Authorization: Bearer` token. The single point every request
    /// routes through, so a request can never silently skip auth.
    fn apply_auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        apply_headers_with_auth(builder, &self.headers, self.auth_server_name.as_deref())
    }

    /// Scan a response body — plain JSON or an SSE `data:` stream — for inbound
    /// notifications the manager acts on, buffering each one.
    fn capture_inbound_notifications(&self, body: &str) {
        let trimmed = body.trim();
        if trimmed.starts_with('{') {
            self.capture_notification_value(trimmed);
            return;
        }
        for line in trimmed.lines() {
            if let Some(data) = line.trim().strip_prefix("data:") {
                self.capture_notification_value(data.trim());
            }
        }
    }

    /// Buffer one frame if it is a notification (no `id`) the manager acts on.
    fn capture_notification_value(&self, data: &str) {
        if !data.starts_with('{') {
            return;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
            return;
        };
        if value.get("id").is_some() {
            return; // a response, not a notification
        }
        if let Some(event) = inbound_event_for_notification(&value) {
            self.inbound.push(event);
        }
    }

    /// Drain inbound events captured from response bodies since the last poll.
    pub fn poll_inbound(&mut self) -> Vec<InboundEvent> {
        self.inbound.drain()
    }

    /// Store the session id echoed by the server, skipping the write when it is
    /// unchanged — a server re-sends the same `Mcp-Session-Id` on every reply, so
    /// the steady state avoids both a redundant `String` allocation and a store.
    fn set_session_id(&self, id: &str) {
        let mut guard = self
            .session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.as_deref() != Some(id) {
            *guard = Some(id.to_string());
        }
    }
}

/// Read a response body to a `String`, rejecting one that would exceed the
/// shared MCP message cap. `reqwest::Response::text` is unbounded, so a server
/// (compromised or buggy) could otherwise stream an arbitrarily large body into
/// memory. Streaming chunks lets the guard reject before the whole body lands.
async fn read_bounded_body(response: reqwest::Response) -> io::Result<String> {
    read_bounded_body_capped(response, MAX_MCP_MESSAGE_BYTES).await
}

/// Chunk-streaming core of [`read_bounded_body`], with the byte cap injected so a
/// test can exercise the rejection path with a tiny limit instead of allocating
/// 32 MiB. Production always calls it with the single-sourced
/// [`MAX_MCP_MESSAGE_BYTES`], so the policy still lives in one place.
async fn read_bounded_body_capped(
    mut response: reqwest::Response,
    cap: usize,
) -> io::Result<String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
    {
        crate::mcp_limits::guard_mcp_read_growth_capped(
            body.len(),
            chunk.len(),
            "HTTP response body",
            cap,
        )?;
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Connect to an MCP server over Streamable HTTP transport.
///
/// This is the HTTP counterpart to [`crate::mcp_stdio::spawn_mcp_stdio_process`]
/// and [`crate::mcp_sse::connect_mcp_sse`].
pub fn connect_mcp_http(
    transport: &McpRemoteTransport,
    auth_server_name: Option<String>,
) -> io::Result<McpHttpProcess> {
    McpHttpProcess::connect(transport, auth_server_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_reads_a_chunked_response_body_through_the_bounded_reader() {
        // Transport-owning coverage of `read_bounded_body`: a real POST is
        // answered by a mock server that flushes the JSON-RPC body in two writes,
        // so the streaming chunk loop must reassemble it. This drives the bounded
        // reader end to end rather than the pure `guard_mcp_read_growth` unit.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping HTTP test: listener bind not permitted here");
                return;
            }
            Err(error) => panic!("bind: {error}"),
        };
        let addr = listener.local_addr().expect("addr");

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf).await;
            let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            // Flush the head+first half, then the rest, so the response arrives
            // in multiple chunks the bounded reader must stitch together.
            let (first, second) = body.split_at(body.len() / 2);
            stream.write_all(head.as_bytes()).await.expect("head");
            stream.write_all(first.as_bytes()).await.expect("first");
            stream.flush().await.expect("flush");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            stream.write_all(second.as_bytes()).await.expect("second");
            stream.flush().await.expect("flush2");
        });

        let transport = McpRemoteTransport {
            url: format!("http://{addr}/rpc"),
            headers: BTreeMap::new(),
            headers_helper: None,
            auth: crate::mcp_client::McpClientAuth::None,
        };
        let process = McpHttpProcess::connect(&transport, None).expect("connect");

        let response: JsonRpcResponse<serde_json::Value> = process
            .request(JsonRpcId::Number(1), "ping", None::<()>)
            .await
            .expect("bounded read should reassemble the chunked body");
        assert_eq!(response.id, JsonRpcId::Number(1));
        assert_eq!(response.result.and_then(|r| r.get("ok").cloned()), Some(serde_json::json!(true)));

        server.abort();
    }

    #[tokio::test]
    async fn read_bounded_body_rejects_a_body_past_the_cap() {
        // Directly exercises `read_bounded_body`'s rejection path via the tiny-cap
        // seam, so no 32 MiB is allocated. A mock server returns a body larger
        // than the injected cap; the bounded reader must reject it (InvalidData)
        // rather than buffering it all.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping HTTP test: listener bind not permitted here");
                return;
            }
            Err(error) => panic!("bind: {error}"),
        };
        let addr = listener.local_addr().expect("addr");

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let body = "a".repeat(4096);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.expect("head");
            stream.write_all(body.as_bytes()).await.expect("body");
            stream.flush().await.expect("flush");
        });

        let response = reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("request");
        // Cap far below the 4096-byte body so the reader rejects it.
        let error = read_bounded_body_capped(response, 64)
            .await
            .expect_err("an over-cap body must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("HTTP response body"),
            "error should name the read site: {error}"
        );

        server.abort();
    }

    #[test]
    fn creates_http_process_from_transport() {
        let transport = McpRemoteTransport {
            url: "https://mcp.example.com/rpc".to_string(),
            headers: BTreeMap::from([("X-Api-Key".to_string(), "test-key".to_string())]),
            headers_helper: None,
            auth: crate::mcp_client::McpClientAuth::None,
        };

        let process = McpHttpProcess::connect(&transport, None).unwrap();
        assert_eq!(process.url(), "https://mcp.example.com/rpc");
        assert!(process.current_session_id().is_none());
    }

    #[test]
    fn session_id_round_trips() {
        let transport = McpRemoteTransport {
            url: "https://mcp.example.com/rpc".to_string(),
            headers: BTreeMap::new(),
            headers_helper: None,
            auth: crate::mcp_client::McpClientAuth::None,
        };

        let process = McpHttpProcess::connect(&transport, None).unwrap();
        assert!(process.current_session_id().is_none());

        process.set_session_id("session-42");
        assert_eq!(process.current_session_id().as_deref(), Some("session-42"));

        // A repeated identical id is a no-op (no redundant store).
        process.set_session_id("session-42");
        assert_eq!(process.current_session_id().as_deref(), Some("session-42"));
    }

    fn http_process() -> McpHttpProcess {
        McpHttpProcess::connect(
            &McpRemoteTransport {
                url: "https://mcp.example.com/rpc".to_string(),
                headers: BTreeMap::new(),
                headers_helper: None,
                auth: crate::mcp_client::McpClientAuth::None,
            },
            None,
        )
        .unwrap()
    }

    #[test]
    fn surfaces_tools_list_changed_interleaved_in_a_response_body() {
        // Regression: a Streamable-HTTP server may emit `tools/list_changed` in
        // the SSE frames of a POST reply. Previously HTTP dropped every inbound
        // notification, so a mid-session tool change was never picked up.
        let mut process = http_process();
        let body =
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
        process.capture_inbound_notifications(body);
        assert_eq!(process.poll_inbound(), vec![InboundEvent::ToolsListChanged]);
        assert!(
            process.poll_inbound().is_empty(),
            "the buffer drains on poll"
        );
    }

    #[test]
    fn response_only_body_surfaces_nothing_and_notifications_dedup() {
        let mut process = http_process();
        // A plain response (has an `id`) is not a notification.
        process.capture_inbound_notifications(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        assert!(process.poll_inbound().is_empty());
        // Repeated `tools/list_changed` collapse to one (an idempotent refresh).
        let twice = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n\
                     data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n";
        process.capture_inbound_notifications(twice);
        assert_eq!(process.poll_inbound(), vec![InboundEvent::ToolsListChanged]);
    }
}
