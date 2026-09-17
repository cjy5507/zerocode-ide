//! WebSocket transport for MCP servers.
//!
//! Implements JSON-RPC over WebSocket text frames so the server manager can
//! treat stdio, HTTP, SSE, and WebSocket transports through the same request
//! lifecycle.

use std::collections::BTreeMap;
use std::io;

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::mcp_client::McpRemoteTransport;
use crate::mcp_http_common::resolve_headers_helper;
use crate::mcp_stdio::{
    inbound_event_for_notification, InboundEvent, JsonRpcId, JsonRpcRequest, JsonRpcResponse,
    McpInitializeParams, McpInitializeResult, McpListResourcesParams, McpListResourcesResult,
    McpListToolsParams, McpListToolsResult, McpReadResourceParams, McpReadResourceResult,
    McpToolCallParams, McpToolCallResult,
};

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// An active WebSocket connection to a remote MCP server.
#[derive(Debug)]
pub struct McpWsProcess {
    url: String,
    stream: WsStream,
    /// Inbound notifications (`tools/list_changed`) captured off the socket while
    /// reading responses, drained by [`Self::poll_inbound`] — the same lazy
    /// capture the stdio read loop performs.
    inbound_events: Vec<InboundEvent>,
}

impl McpWsProcess {
    /// Connect to a WebSocket MCP server with configured headers applied.
    ///
    /// `auth_server_name` is the OAuth token's storage key (`Some` only for an
    /// OAuth-authenticated server). Unlike SSE/HTTP, a WebSocket carries auth only
    /// in the opening handshake — frames have no headers — so the bearer is
    /// attached here; an externally refreshed token is picked up on reconnect.
    pub async fn connect(
        transport: &McpRemoteTransport,
        auth_server_name: Option<String>,
    ) -> io::Result<Self> {
        let mut request = transport
            .url
            .clone()
            .into_client_request()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

        let mut merged_headers = transport.headers.clone();
        let helper_headers = resolve_headers_helper(transport.headers_helper.as_deref(), false)?;
        merged_headers.extend(helper_headers);
        if let Some(token) = auth_server_name
            .as_deref()
            .and_then(crate::mcp_oauth::get_mcp_bearer_token)
        {
            merged_headers.insert("Authorization".to_string(), format!("Bearer {token}"));
        }
        apply_ws_headers(request.headers_mut(), &merged_headers)?;

        // Bound the opening handshake: connect_async has no internal timeout, so
        // a host that accepts TCP but stalls the WS/TLS upgrade would hang here
        // forever — no per-RPC timeout covers the connect itself. Cap it so the
        // server surfaces as failed instead of staying pinned to "discovering".
        let (stream, _response) = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connect_async(request),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "WebSocket connect timed out"))?
        .map_err(|error| io::Error::new(io::ErrorKind::ConnectionRefused, error))?;

        Ok(Self {
            url: transport.url.clone(),
            stream,
            inbound_events: Vec::new(),
        })
    }

    /// Record an inbound event, deduplicating so the buffer stays bounded — one
    /// pending `ToolsListChanged` is indistinguishable from ten (a refresh re-reads
    /// the server's current tool set). Mirrors the stdio transport's buffer.
    fn push_inbound(&mut self, event: InboundEvent) {
        if !self.inbound_events.contains(&event) {
            self.inbound_events.push(event);
        }
    }

    /// Drain inbound events captured since the last poll.
    pub fn poll_inbound(&mut self) -> Vec<InboundEvent> {
        std::mem::take(&mut self.inbound_events)
    }

    /// Send a JSON-RPC request and wait for the matching response frame.
    pub async fn request<TParams: Serialize, TResult: DeserializeOwned>(
        &mut self,
        id: JsonRpcId,
        method: impl Into<String>,
        params: Option<TParams>,
    ) -> io::Result<JsonRpcResponse<TResult>> {
        let method = method.into();
        let request = JsonRpcRequest::new(id.clone(), method.clone(), params);
        let payload = serde_json::to_string(&request)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

        self.stream
            .send(Message::Text(payload))
            .await
            .map_err(io::Error::other)?;

        let response: JsonRpcResponse<TResult> = self.read_jsonrpc_response(&method, &id).await?;

        if response.jsonrpc != "2.0" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "WebSocket response for {method} used unsupported jsonrpc version `{}`",
                    response.jsonrpc
                ),
            ));
        }

        if response.id != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "WebSocket response for {method} used mismatched id: expected {id:?}, got {:?}",
                    response.id
                ),
            ));
        }

        Ok(response)
    }

    pub async fn initialize(
        &mut self,
        id: JsonRpcId,
        params: McpInitializeParams,
    ) -> io::Result<JsonRpcResponse<McpInitializeResult>> {
        self.request(id, "initialize", Some(params)).await
    }

    /// Send a JSON-RPC notification (no `id`, no response expected).
    pub async fn send_notification(&mut self, method: &str) -> io::Result<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method
        });
        let payload = serde_json::to_string(&notification)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.stream
            .send(Message::Text(payload))
            .await
            .map_err(io::Error::other)
    }

    pub async fn list_tools(
        &mut self,
        id: JsonRpcId,
        params: Option<McpListToolsParams>,
    ) -> io::Result<JsonRpcResponse<McpListToolsResult>> {
        self.request(id, "tools/list", params).await
    }

    pub async fn call_tool(
        &mut self,
        id: JsonRpcId,
        params: McpToolCallParams,
    ) -> io::Result<JsonRpcResponse<McpToolCallResult>> {
        self.request(id, "tools/call", Some(params)).await
    }

    pub async fn list_resources(
        &mut self,
        id: JsonRpcId,
        params: Option<McpListResourcesParams>,
    ) -> io::Result<JsonRpcResponse<McpListResourcesResult>> {
        self.request(id, "resources/list", params).await
    }

    pub async fn read_resource(
        &mut self,
        id: JsonRpcId,
        params: McpReadResourceParams,
    ) -> io::Result<JsonRpcResponse<McpReadResourceResult>> {
        self.request(id, "resources/read", Some(params)).await
    }

    pub async fn shutdown(&mut self) -> io::Result<()> {
        self.stream.close(None).await.map_err(io::Error::other)
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    async fn read_jsonrpc_response<TResult: DeserializeOwned>(
        &mut self,
        method: &str,
        expected_id: &JsonRpcId,
    ) -> io::Result<JsonRpcResponse<TResult>> {
        let expected = serde_json::to_value(expected_id)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        while let Some(frame) = self.stream.next().await {
            let frame = frame.map_err(io::Error::other)?;
            let payload = match frame {
                Message::Text(text) => text,
                Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Message::Ping(payload) => {
                    self.stream
                        .send(Message::Pong(payload))
                        .await
                        .map_err(io::Error::other)?;
                    continue;
                }
                Message::Pong(_) | Message::Frame(_) => continue,
                Message::Close(frame) => {
                    let detail = frame.map_or_else(
                        || "connection closed".to_string(),
                        |close| close.reason.to_string(),
                    );
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "WebSocket stream for {} closed while waiting for {method} response to {expected_id:?}: {detail}",
                            self.url
                        ),
                    ));
                }
            };

            // The server may interleave notifications (no `id`, e.g.
            // `notifications/progress`) or another request's response before
            // ours; match by id and keep waiting otherwise — the same
            // correlation the stdio transport's read loop performs.
            let value: serde_json::Value = serde_json::from_str(&payload)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            // Correlate by id in a single lookup (mirrors the stdio read loop).
            match value.get("id") {
                // A notification (no `id`): surface `tools/list_changed` so a
                // mid-session tool change is picked up, then keep waiting for our
                // response instead of dropping the frame.
                None => {
                    if let Some(event) = inbound_event_for_notification(&value) {
                        self.push_inbound(event);
                    }
                    continue;
                }
                // Another request's response — keep waiting for ours.
                Some(frame_id) if *frame_id != expected => continue,
                Some(_) => {}
            }
            return serde_json::from_value(value)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        }

        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "WebSocket stream for {} ended before returning {method} response to {expected_id:?}",
                self.url
            ),
        ))
    }
}

fn apply_ws_headers(
    headers: &mut tokio_tungstenite::tungstenite::http::HeaderMap,
    extra_headers: &BTreeMap<String, String>,
) -> io::Result<()> {
    for (name, value) in extra_headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        headers.insert(header_name, header_value);
    }
    Ok(())
}

pub async fn connect_mcp_ws(
    transport: &McpRemoteTransport,
    auth_server_name: Option<String>,
) -> io::Result<McpWsProcess> {
    McpWsProcess::connect(transport, auth_server_name).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio_tungstenite::accept_async;

    use super::*;
    use crate::mcp_client::McpClientAuth;

    fn ws_transport(url: &str) -> McpRemoteTransport {
        McpRemoteTransport {
            url: url.to_string(),
            headers: BTreeMap::new(),
            headers_helper: None,
            auth: McpClientAuth::None,
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn connect_and_round_trip_jsonrpc_requests() {
        let listener = match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "skipping websocket integration test: listener bind is not permitted in this environment"
                );
                return;
            }
            Err(error) => panic!("bind: {error}"),
        };
        let addr = listener.local_addr().expect("addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let ws = accept_async(stream).await.expect("handshake");
            let (mut writer, mut reader) = ws.split();

            while let Some(message) = reader.next().await {
                let message = message.expect("message");
                let Message::Text(text) = message else {
                    continue;
                };
                let request: serde_json::Value = serde_json::from_str(&text).expect("json");
                let method = request["method"].as_str().expect("method");
                let id = request["id"].clone();

                let response = match method {
                    "initialize" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": request["params"]["protocolVersion"],
                            "capabilities": { "tools": {}, "resources": {} },
                            "serverInfo": { "name": "ws-test", "version": "1.0.0" }
                        }
                    }),
                    "tools/list" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "tools": [{
                                "name": "echo",
                                "description": "echo tool",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": { "text": { "type": "string" } },
                                    "required": ["text"]
                                }
                            }],
                            "nextCursor": null
                        }
                    }),
                    "tools/call" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{
                                "type": "text",
                                "text": format!("echo:{}", request["params"]["arguments"]["text"].as_str().unwrap_or_default())
                            }],
                            "structuredContent": { "ok": true },
                            "isError": false
                        }
                    }),
                    _ => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "unknown method" }
                    }),
                };

                writer
                    .send(Message::Text(response.to_string()))
                    .await
                    .expect("send");
            }

            let _ = shutdown_rx.await;
        });

        let mut process = connect_mcp_ws(&ws_transport(&format!("ws://{addr}")), None)
            .await
            .expect("connect");
        let init = process
            .initialize(
                JsonRpcId::Number(1),
                McpInitializeParams {
                    protocol_version: "2025-03-26".to_string(),
                    capabilities: json!({}),
                    client_info: crate::McpInitializeClientInfo {
                        name: "test".to_string(),
                        version: "0.1.0".to_string(),
                    },
                },
            )
            .await
            .expect("initialize");
        assert_eq!(
            init.result.expect("init result").server_info.name,
            "ws-test"
        );

        let tools = process
            .list_tools(
                JsonRpcId::Number(2),
                Some(McpListToolsParams { cursor: None }),
            )
            .await
            .expect("tools");
        assert_eq!(tools.result.expect("tools result").tools[0].name, "echo");

        let call = process
            .call_tool(
                JsonRpcId::Number(3),
                McpToolCallParams {
                    name: "echo".to_string(),
                    arguments: Some(json!({ "text": "hello" })),
                    meta: None,
                },
            )
            .await
            .expect("call");
        assert_eq!(
            call.result.expect("call result").content[0].data["text"],
            json!("echo:hello")
        );

        process.shutdown().await.expect("shutdown");
        let _ = shutdown_tx.send(());
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn connect_rejects_invalid_header_values() {
        let mut transport = ws_transport("ws://127.0.0.1:9");
        transport
            .headers
            .insert("X-Test".to_string(), "bad\nvalue".to_string());

        let error = connect_mcp_ws(&transport, None)
            .await
            .expect_err("invalid header should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn call_tool_skips_notifications_before_the_response() {
        // Regression: a server may stream a `notifications/progress` frame (no
        // `id`) before the actual response. The read loop must skip it and
        // correlate the response by id rather than returning the first frame.
        let listener = match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping websocket test: listener bind not permitted here");
                return;
            }
            Err(error) => panic!("bind: {error}"),
        };
        let addr = listener.local_addr().expect("addr");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let ws = accept_async(stream).await.expect("handshake");
            let (mut writer, mut reader) = ws.split();

            if let Some(Ok(Message::Text(text))) = reader.next().await {
                let request: serde_json::Value = serde_json::from_str(&text).expect("json");
                let id = request["id"].clone();
                // Interleave a notification (no id) BEFORE the response.
                writer
                    .send(Message::Text(
                        json!({
                            "jsonrpc": "2.0",
                            "method": "notifications/progress",
                            "params": { "progress": 1, "total": 2 }
                        })
                        .to_string(),
                    ))
                    .await
                    .expect("send notification");
                writer
                    .send(Message::Text(
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{ "type": "text", "text": "done" }],
                                "isError": false
                            }
                        })
                        .to_string(),
                    ))
                    .await
                    .expect("send response");
            }
            let _ = reader.next().await;
        });

        let mut process = connect_mcp_ws(&ws_transport(&format!("ws://{addr}")), None)
            .await
            .expect("connect");
        let call = process
            .call_tool(
                JsonRpcId::Number(7),
                McpToolCallParams {
                    name: "echo".to_string(),
                    arguments: Some(json!({})),
                    meta: None,
                },
            )
            .await
            .expect("call must skip the leading notification and return the response");
        assert_eq!(call.id, JsonRpcId::Number(7));
        assert_eq!(
            call.result.expect("call result").content[0].data["text"],
            json!("done")
        );

        process.shutdown().await.expect("shutdown");
        let _ = server.await;
    }

    #[tokio::test]
    async fn surfaces_tools_list_changed_notification_via_poll_inbound() {
        // A server that announces a mid-session tool-set change must have that
        // `tools/list_changed` surfaced (so the manager re-discovers), not dropped.
        let listener = match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping websocket test: listener bind not permitted here");
                return;
            }
            Err(error) => panic!("bind: {error}"),
        };
        let addr = listener.local_addr().expect("addr");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let ws = accept_async(stream).await.expect("handshake");
            let (mut writer, mut reader) = ws.split();
            if let Some(Ok(Message::Text(text))) = reader.next().await {
                let request: serde_json::Value = serde_json::from_str(&text).expect("json");
                let id = request["id"].clone();
                writer
                    .send(Message::Text(
                        json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })
                            .to_string(),
                    ))
                    .await
                    .expect("send notification");
                writer
                    .send(Message::Text(
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [] } })
                            .to_string(),
                    ))
                    .await
                    .expect("send response");
            }
            let _ = reader.next().await;
        });

        let mut process = connect_mcp_ws(&ws_transport(&format!("ws://{addr}")), None)
            .await
            .expect("connect");
        process
            .call_tool(
                JsonRpcId::Number(1),
                McpToolCallParams {
                    name: "echo".to_string(),
                    arguments: Some(json!({})),
                    meta: None,
                },
            )
            .await
            .expect("call");

        assert_eq!(process.poll_inbound(), vec![InboundEvent::ToolsListChanged]);
        assert!(
            process.poll_inbound().is_empty(),
            "the buffer drains on poll"
        );

        process.shutdown().await.expect("shutdown");
        let _ = server.await;
    }
}
