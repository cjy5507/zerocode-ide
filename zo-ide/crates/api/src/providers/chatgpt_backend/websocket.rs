//! Responses over a WebSocket — the transport codex prefers for the ChatGPT
//! backend (`prefer_websockets: true`, `OpenAI-Beta: responses_websockets=…`).
//!
//! The SSE stream that carried a gpt-5.6 turn on 2026-09-04 went eight minutes
//! with keepalives and no first token, then the backend dropped it and the turn
//! restarted from a retry. Over a WebSocket the client answers the server's
//! pings, so no proxy on the way sees an idle connection, and the frames are
//! the SAME JSON events the SSE body carries — one text frame per event — so
//! everything downstream of the transport (`ResponsesStreamState`) is shared.
//!
//! What this module does not do yet: reuse one connection across requests with
//! `previous_response_id` (codex's incremental payloads). Every request opens a
//! connection and closes it after the response completes.

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::error::ApiError;

/// The `OpenAI-Beta` value that opts the handshake into the Responses
/// WebSocket surface — the one the installed codex (0.153.1) sends.
pub(super) const OPENAI_BETA_RESPONSES_WEBSOCKETS: &str = "responses_websockets=2026-02-06";

/// `ZO_CHATGPT_TRANSPORT`: `auto` (default — WebSocket for an https backend,
/// SSE for anything else, SSE for the rest of the session once a handshake
/// fails), `ws` (WebSocket, no fallback), `sse`.
pub(super) const TRANSPORT_ENV: &str = "ZO_CHATGPT_TRANSPORT";

/// Frame `type` of the request that opens a response on the socket.
const RESPONSE_CREATE: &str = "response.create";
/// The server no longer holds the response a continuation named — codex's
/// "Previous response was not found. Retrying the full request."
const PREVIOUS_RESPONSE_NOT_FOUND: &str = "previous_response_not_found";
/// How long a connection whose response completed is kept for the next
/// request. Nobody answers the server's pings between requests, so an idle
/// socket is presumed gone after this; a tool round usually returns sooner,
/// and a socket that is gone anyway is caught on the first read
/// (`ChatGptStream::reopen_if_continuation_failed`).
pub(super) const HELD_CONNECTION_MAX_IDLE: Duration = Duration::from_secs(60);

/// Error codes the server sends as `error` frames that mean "open a fresh
/// connection and try again", not "this request is wrong". The plan window
/// (`usage_limit_reached`) is deliberately absent: it is this account's 429
/// (see [`super::usage_limit_error`]), and a fresh connection meets the same
/// wall.
const RETRYABLE_ERROR_CODES: &[&str] = &[
    "websocket_connection_limit_reached",
    PREVIOUS_RESPONSE_NOT_FOUND,
    "rate_limit_exceeded",
    "server_error",
    "service_unavailable",
    "overloaded",
];

/// How the ChatGPT backend's response is carried; see [`TRANSPORT_ENV`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Auto,
    Sse,
    Websocket,
}

/// The operator's transport choice, read per call so the knob works without a
/// rebuild — the escape-hatch idiom of the other `ZO_*` stream knobs.
pub(super) fn transport_choice() -> Transport {
    match std::env::var(TRANSPORT_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("sse" | "http" | "https") => Transport::Sse,
        Some("ws" | "wss" | "websocket") => Transport::Websocket,
        _ => Transport::Auto,
    }
}

/// The socket URL for a Responses endpoint: the same host and path, `wss` for
/// `https` and `ws` for `http`. `None` for a scheme with no socket form.
#[must_use]
pub(super) fn websocket_url(base_url: &str) -> Option<String> {
    base_url
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| base_url.strip_prefix("http://").map(|rest| format!("ws://{rest}")))
}

/// One `response.create` frame — the body's properties, the items riding this
/// frame, and for a continuation the response they follow. Serialized from
/// borrows: the body is not copied to be sent.
#[derive(serde::Serialize)]
struct CreateFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    properties: &'a Value,
    input: &'a [Value],
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<&'a str>,
}

/// What a `response.create` frame said, kept so the connection can carry the
/// next request as a continuation once this response completes.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SentRequest {
    /// The body minus `input` — model, instructions, tools, … — which must be
    /// unchanged for a `previous_response_id` to name the same context.
    properties: Value,
    /// The full `input` zo built for this request: what the server holds up to
    /// it, whether the frame carried all of it or only the new items.
    input: Vec<Value>,
    /// The frame named a `previous_response_id` and carried only new items.
    incremental: bool,
}

/// A connection kept after its response completed: the socket, the response
/// the server will continue from, and what the server holds.
#[derive(Debug)]
pub(super) struct HeldConnection {
    socket: ResponsesWebsocket,
    response_id: String,
    sent: SentRequest,
    /// The response's output items as the server reported them
    /// (`response.output_item.done`).
    output: Vec<Value>,
    finished_at: Instant,
}

impl HeldConnection {
    /// Send `body` — a full request zo built — as the continuation of this
    /// connection's response, or give the body back: when the connection has
    /// sat idle too long, when the request does not extend what the server
    /// holds (the socket is dropped, which closes it), or when the socket
    /// refused the frame.
    pub(super) async fn continue_with(self, body: Value) -> Result<ResponsesWebsocket, Value> {
        if self.finished_at.elapsed() > HELD_CONNECTION_MAX_IDLE {
            return Err(body);
        }
        let Some(delta) = delta_after(&self.sent, &self.output, &body) else {
            return Err(body);
        };
        let Self {
            mut socket,
            response_id,
            ..
        } = self;
        socket
            .send_create_after(&response_id, body, delta)
            .await
            .map(|()| socket)
    }
}

/// The items of `body` to send after the response `sent` produced, or `None`
/// when `body` is not an extension of what the server holds: everything but
/// `input` must equal `sent`'s, `input` must begin with `sent`'s input followed
/// by items that say what the response's `output` said (reasoning items are
/// compared on neither side — zo replays them from its own cache), and
/// something new must follow.
fn delta_after(sent: &SentRequest, output: &[Value], body: &Value) -> Option<Vec<Value>> {
    let input = body.get("input")?.as_array()?;
    if properties_of(body) != sent.properties {
        return None;
    }
    let held = sent.input.len();
    if input.len() < held || input[..held] != sent.input[..] {
        return None;
    }
    let rest = &input[held..];
    let replayed = output_span(output, rest)?;
    let delta = &rest[replayed..];
    (!delta.is_empty()).then(|| delta.to_vec())
}

/// How many leading items of `rebuilt` — a response's output as zo rebuilt it
/// from its transcript — restate `output`, the response's output as the server
/// reported it. Items are compared by what they say ([`canonical_item`]);
/// reasoning items count on neither side, and reasoning zo replays right
/// after the restated items belongs to the response too.
fn output_span(output: &[Value], rebuilt: &[Value]) -> Option<usize> {
    let mut expected = output.iter().filter_map(canonical_item);
    let mut span = 0;
    let mut want = expected.next();
    while let Some(expected_item) = &want {
        let item = rebuilt.get(span)?;
        span += 1;
        match canonical_item(item) {
            None => {}
            Some(got) if got == *expected_item => want = expected.next(),
            Some(_) => return None,
        }
    }
    while rebuilt
        .get(span)
        .is_some_and(|item| canonical_item(item).is_none())
    {
        span += 1;
    }
    Some(span)
}

/// What an item says, so zo's rebuilt item can be compared with the server's:
/// a function call is its call id, name and arguments (as JSON, not as the
/// text the model typed), a message is its role and text, reasoning is
/// nothing (`None` — not compared), anything else is itself without the
/// server's ids and statuses.
fn canonical_item(item: &Value) -> Option<Value> {
    match item.get("type").and_then(Value::as_str) {
        Some("reasoning") => None,
        Some("function_call") => {
            let arguments = item.get("arguments").map(|arguments| match arguments {
                Value::String(text) => {
                    serde_json::from_str::<Value>(text).unwrap_or_else(|_| arguments.clone())
                }
                other => other.clone(),
            });
            Some(json!({
                "type": "function_call",
                "call_id": item.get("call_id"),
                "name": item.get("name"),
                "arguments": arguments,
            }))
        }
        Some("message") => {
            let text: String = item
                .get("content")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect()
                })
                .unwrap_or_default();
            Some(json!({
                "type": "message",
                "role": item.get("role"),
                "text": text,
            }))
        }
        _ => {
            let mut item = item.clone();
            if let Value::Object(map) = &mut item {
                map.remove("id");
                map.remove("status");
            }
            Some(item)
        }
    }
}

/// `body` without its `input`.
fn properties_of(body: &Value) -> Value {
    match body {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "input")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `body` split into its properties and its `input` items.
fn split_body(mut body: Value) -> (Value, Vec<Value>) {
    let input = match body.as_object_mut().and_then(|map| map.remove("input")) {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    (body, input)
}

/// The inverse of [`split_body`].
fn join_body(mut properties: Value, input: Vec<Value>) -> Value {
    if let Value::Object(map) = &mut properties {
        map.insert("input".to_string(), Value::Array(input));
    }
    properties
}

/// One Responses WebSocket: the socket, what its last `response.create`
/// said, and what the response has reported so far.
pub(super) struct ResponsesWebsocket {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    sent: Option<SentRequest>,
    output: Vec<Value>,
    response_id: Option<String>,
    progressed: bool,
}

impl std::fmt::Debug for ResponsesWebsocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponsesWebsocket")
            .field(
                "sent_incrementally",
                &self.sent.as_ref().map(|sent| sent.incremental),
            )
            .field("output_items", &self.output.len())
            .field("response_id", &self.response_id)
            .field("progressed", &self.progressed)
            .finish_non_exhaustive()
    }
}

impl ResponsesWebsocket {
    pub(super) async fn connect(url: &str, headers: &[(&str, String)]) -> Result<Self, ApiError> {
        let mut request = url
            .into_client_request()
            .map_err(|error| handshake_error(format!("invalid websocket request: {error}")))?;
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| handshake_error(format!("invalid header {name}: {error}")))?;
            let value = HeaderValue::from_str(value)
                .map_err(|error| handshake_error(format!("invalid header {name}: {error}")))?;
            request.headers_mut().insert(name, value);
        }
        let (stream, _response) = connect_async(request).await.map_err(|error| ws_error(&error))?;
        Ok(Self {
            stream,
            sent: None,
            output: Vec::new(),
            response_id: None,
            progressed: false,
        })
    }

    /// Send `body` as a `response.create` carrying all of it.
    pub(super) async fn send_create(&mut self, body: Value) -> Result<(), ApiError> {
        let (properties, input) = split_body(body);
        self.send_frame(&properties, &input, None).await?;
        self.sent = Some(SentRequest {
            properties,
            input,
            incremental: false,
        });
        Ok(())
    }

    /// Send `body` as the continuation of `previous_response_id`: only `delta`
    /// rides the frame. When the socket refuses the frame the body comes back
    /// whole, for a fresh connection.
    async fn send_create_after(
        &mut self,
        previous_response_id: &str,
        body: Value,
        delta: Vec<Value>,
    ) -> Result<(), Value> {
        let (properties, input) = split_body(body);
        match self
            .send_frame(&properties, &delta, Some(previous_response_id))
            .await
        {
            Ok(()) => {
                self.sent = Some(SentRequest {
                    properties,
                    input,
                    incremental: true,
                });
                Ok(())
            }
            Err(_refused) => Err(join_body(properties, input)),
        }
    }

    async fn send_frame(
        &mut self,
        properties: &Value,
        input: &[Value],
        previous_response_id: Option<&str>,
    ) -> Result<(), ApiError> {
        let frame = serde_json::to_string(&CreateFrame {
            kind: RESPONSE_CREATE,
            properties,
            input,
            previous_response_id,
        })?;
        self.output.clear();
        self.response_id = None;
        self.progressed = false;
        self.stream
            .send(Message::Text(frame))
            .await
            .map_err(|error| ws_error(&error))
    }

    /// The last frame was a continuation (`previous_response_id`).
    pub(super) fn sent_incrementally(&self) -> bool {
        self.sent.as_ref().is_some_and(|sent| sent.incremental)
    }

    /// The server has said something since the last frame went out.
    pub(super) fn has_progressed(&self) -> bool {
        self.progressed
    }

    /// The response is over: the connection is kept for the next request when
    /// the response completed with an id to continue from; otherwise the
    /// socket is closed.
    pub(super) async fn finish(mut self) -> Option<HeldConnection> {
        if let (Some(sent), Some(response_id)) = (self.sent.take(), self.response_id.take()) {
            let output = std::mem::take(&mut self.output);
            return Some(HeldConnection {
                socket: self,
                response_id,
                sent,
                output,
                finished_at: Instant::now(),
            });
        }
        self.close().await;
        None
    }

    pub(super) async fn next_batch(&mut self) -> Result<Option<Vec<Value>>, ApiError> {
        loop {
            let Some(message) = self.stream.next().await else {
                return Ok(None);
            };
            match message.map_err(|error| ws_error(&error))? {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text)?;
                    if let Some(error) = error_frame(&value) {
                        return Err(error);
                    }
                    self.progressed = true;
                    self.note(&value);
                    return Ok(Some(vec![value]));
                }
                Message::Ping(payload) => {
                    self.stream
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|error| ws_error(&error))?;
                    return Ok(Some(Vec::new()));
                }
                Message::Close(_) => return Ok(None),
                Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
            }
        }
    }

    /// Remember what the next request will need: the response's output items
    /// as the server reported them, and the id a continuation names.
    fn note(&mut self, value: &Value) {
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => {
                if let Some(item) = value.get("item") {
                    self.output.push(item.clone());
                }
            }
            Some("response.completed") => {
                self.response_id = value
                    .pointer("/response/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            _ => {}
        }
    }

    pub(super) async fn close(&mut self) {
        let _ = self.stream.close(None).await;
    }
}

/// Whether `value` is a response's terminal event — after it the server keeps
/// the socket open for the next request, so the stream must end itself.
#[must_use]
pub(super) fn is_terminal_event(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("response.completed" | "response.failed" | "response.incomplete")
    )
}

/// An `error` frame, as the error it names: `{ "type": "error", "error": {
/// "code", "message" }, "status"? }`. Codes in [`RETRYABLE_ERROR_CODES`] and
/// 5xx/429 statuses are retryable; the rest are the request's own fault.
/// The server's answer to a continuation whose previous response it no
/// longer holds — the full request is sent instead, at once.
#[must_use]
pub(super) fn is_previous_response_not_found(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::StreamApi {
            error_type: Some(code),
            ..
        } if code == PREVIOUS_RESPONSE_NOT_FOUND
    )
}

fn error_frame(value: &Value) -> Option<ApiError> {
    error_frame_with_reset(value, super::measured_usage_reset)
}

/// [`error_frame`] with the measured-window reader injected, so a test can
/// say what zo has measured without touching the process-wide quota state.
fn error_frame_with_reset(
    value: &Value,
    measured: impl FnOnce() -> Option<Duration>,
) -> Option<ApiError> {
    if value.get("type").and_then(Value::as_str) != Some("error") {
        return None;
    }
    let error = value.get("error");
    let code = error
        .and_then(|error| {
            error
                .get("code")
                .and_then(Value::as_str)
                .or_else(|| error.get("type").and_then(Value::as_str))
        })
        .map(str::to_string);
    let message = error
        .and_then(|error| error.get("message"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if code.as_deref() == Some(super::USAGE_LIMIT_CODE) {
        return Some(super::usage_limit_error(
            message,
            value.to_string(),
            super::usage_limit_reset(value, measured),
        ));
    }
    let status = value
        .get("status")
        .or_else(|| value.get("status_code"))
        .and_then(Value::as_u64);
    let retryable = code
        .as_deref()
        .is_some_and(|code| RETRYABLE_ERROR_CODES.contains(&code))
        || status.is_some_and(|status| status == 429 || status >= 500);
    Some(ApiError::StreamApi {
        error_type: code,
        message,
        body: value.to_string(),
        retryable,
    })
}

fn handshake_error(message: String) -> ApiError {
    ApiError::StreamApi {
        error_type: Some("websocket_handshake".to_string()),
        message: Some(message),
        body: String::new(),
        retryable: false,
    }
}

/// A socket-level failure is a transport hiccup, retryable like a reset
/// mid-SSE-stream (see `is_transient_transport_error`).
fn ws_error(error: &tokio_tungstenite::tungstenite::Error) -> ApiError {
    ApiError::StreamApi {
        error_type: Some("websocket_transport".to_string()),
        message: Some(error.to_string()),
        body: String::new(),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        SentRequest, canonical_item, delta_after, error_frame, error_frame_with_reset,
        is_terminal_event, output_span, websocket_url,
    };

    #[test]
    fn the_socket_url_keeps_host_and_path_and_swaps_the_scheme() {
        assert_eq!(
            websocket_url("https://chatgpt.com/backend-api/codex/responses").as_deref(),
            Some("wss://chatgpt.com/backend-api/codex/responses")
        );
        assert_eq!(websocket_url("http://127.0.0.1:8/v1").as_deref(), Some("ws://127.0.0.1:8/v1"));
        assert_eq!(websocket_url("file:///nope"), None);
    }

    #[test]
    fn an_error_frame_is_the_error_it_names_and_only_some_are_retryable() {
        let limit = error_frame(&json!({"type":"error","error":{"code":"websocket_connection_limit_reached","message":"Responses websocket connection limit reached (60 minutes)."}})).expect("error");
        assert!(limit.is_retryable(), "a connection limit is a fresh connection away");
        let bad = error_frame(&json!({"type":"error","error":{"code":"invalid_request_error","message":"bad input"}})).expect("error");
        assert!(!bad.is_retryable());
        assert!(bad.to_string().contains("bad input"));
        let overloaded = error_frame(&json!({"type":"error","status":503,"message":"try later"})).expect("error");
        assert!(overloaded.is_retryable());
        assert!(error_frame(&json!({"type":"response.created"})).is_none());
    }

    /// The plan window is not a fresh connection away: a `usage_limit_reached`
    /// frame is this account's 429, carries the reset the server named, and is
    /// never reconnected in-stream (seven reconnects and a dead turn was the
    /// reported behaviour). A burst throttle (`rate_limit_exceeded`) still is.
    #[test]
    fn a_usage_limit_frame_is_this_accounts_window_with_its_reset_not_a_reconnect() {
        let limit = error_frame_with_reset(
            &json!({"type":"error","error":{"code":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"plus","resets_in_seconds":8220}}),
            || Some(std::time::Duration::from_secs(99)),
        )
        .expect("error");
        assert!(!limit.is_retryable(), "a plan window is not a reconnect away");
        assert_eq!(
            limit.provider_error_class(),
            crate::ProviderErrorClass::account_rate_limit(Some(std::time::Duration::from_secs(8220))),
            "the frame's own reset wins over the measured one"
        );
        let text = limit.to_string();
        assert!(text.contains("429"), "text: {text}");
        assert!(text.contains("usage_limit_reached"), "text: {text}");
        assert!(text.contains("retry-after: 8220"), "text: {text}");

        let measured = error_frame_with_reset(
            &json!({"type":"error","error":{"code":"usage_limit_reached","message":"The usage limit has been reached"}}),
            || Some(std::time::Duration::from_secs(99)),
        )
        .expect("error");
        assert_eq!(
            measured.provider_error_class(),
            crate::ProviderErrorClass::account_rate_limit(Some(std::time::Duration::from_secs(99))),
            "a frame that names no reset falls back to the measured window"
        );

        let burst = error_frame(&json!({"type":"error","error":{"code":"rate_limit_exceeded","message":"slow down"}}))
            .expect("error");
        assert!(burst.is_retryable(), "a burst throttle is still reconnected");

        let null_code = error_frame(&json!({
            "type": "error",
            "status": 400,
            "error": {
                "code": null,
                "message": "Model 'gpt-5.3-codex-spark' does not support image inputs.",
                "param": "input",
                "type": "invalid_request_error"
            }
        }))
        .expect("error");
        let text = null_code.to_string();
        assert!(
            text.contains("invalid_request_error"),
            "expected type extracted when code is null, got: {text}"
        );
        assert!(
            text.contains("does not support image inputs"),
            "expected message preserved, got: {text}"
        );
    }

    #[test]
    fn a_response_ends_at_its_terminal_event_not_at_the_socket_close() {
        assert!(is_terminal_event(&json!({"type":"response.completed"})));
        assert!(is_terminal_event(&json!({"type":"response.failed"})));
        assert!(!is_terminal_event(&json!({"type":"response.output_text.delta"})));
    }

    fn sent(input: Vec<serde_json::Value>) -> SentRequest {
        SentRequest {
            properties: json!({"model":"gpt-5.6-sol","instructions":"be brief","tools":[{"type":"function","name":"read"}]}),
            input,
            incremental: false,
        }
    }

    fn body(input: Vec<serde_json::Value>) -> serde_json::Value {
        let mut body = json!({"model":"gpt-5.6-sol","instructions":"be brief","tools":[{"type":"function","name":"read"}]});
        body["input"] = serde_json::Value::Array(input);
        body
    }

    /// A tool round is the previous input, the call as zo rebuilt it (the
    /// arguments re-serialized in zo's key order, zo's replayed reasoning in
    /// front of it), then the tool result — and only the result is new.
    #[test]
    fn a_tool_round_continues_with_only_the_tool_result() {
        let user = json!({"type":"message","role":"user","content":[{"type":"input_text","text":"read x"}]});
        let held = sent(vec![user.clone()]);
        let output = [
            json!({"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque"}),
            json!({"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{\"path\": \"x\", \"lines\": 3}","status":"completed"}),
        ];
        let result = json!({"type":"function_call_output","call_id":"call_1","output":"data"});
        let rebuilt_call = json!({"type":"function_call","call_id":"call_1","name":"read","arguments":"{\"lines\":3,\"path\":\"x\"}"});
        let replayed = json!({"type":"reasoning","id":"rs_1","encrypted_content":"opaque"});
        assert_eq!(
            delta_after(&held, &output, &body(vec![user.clone(), replayed, rebuilt_call.clone(), result.clone()])),
            Some(vec![result.clone()]),
            "the replayed reasoning and the restated call are the response; the result is new"
        );
        assert_eq!(
            delta_after(&held, &output, &body(vec![user.clone(), rebuilt_call.clone(), result.clone()])),
            Some(vec![result.clone()]),
            "zo may replay no reasoning at all"
        );
        assert_eq!(
            delta_after(&held, &output, &body(vec![user.clone(), rebuilt_call.clone()])),
            None,
            "nothing new to send"
        );
        let other_call = json!({"type":"function_call","call_id":"call_9","name":"read","arguments":"{}"});
        assert_eq!(
            delta_after(&held, &output, &body(vec![user.clone(), other_call, result.clone()])),
            None,
            "a call the response did not make is not a continuation"
        );
        assert_eq!(
            delta_after(&held, &output, &body(vec![json!({"type":"message","role":"user","content":[{"type":"input_text","text":"changed"}]}), rebuilt_call.clone(), result.clone()])),
            None,
            "a changed prefix is a different conversation"
        );
        let mut retooled = body(vec![user, rebuilt_call, result]);
        retooled["tools"] = json!([]);
        assert_eq!(delta_after(&held, &output, &retooled), None, "changed properties");
    }

    /// The next turn after a final answer continues too: the answer as zo
    /// rebuilt it says what the server's message item said.
    #[test]
    fn the_next_turn_continues_after_the_answer() {
        let user = json!({"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]});
        let held = sent(vec![user.clone()]);
        let output = [json!({"type":"message","id":"msg_1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Hello","annotations":[]}]})];
        let rebuilt = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello"}]});
        let next = json!({"type":"message","role":"user","content":[{"type":"input_text","text":"and then?"}]});
        assert_eq!(
            delta_after(&held, &output, &body(vec![user.clone(), rebuilt, next.clone()])),
            Some(vec![next.clone()])
        );
        let different = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello there"}]});
        assert_eq!(delta_after(&held, &output, &body(vec![user, different, next])), None);
    }

    #[test]
    fn items_are_compared_by_what_they_say() {
        assert_eq!(
            canonical_item(&json!({"type":"function_call","id":"fc","status":"completed","call_id":"c","name":"n","arguments":"{\"a\": 1, \"b\": [2]}"})),
            canonical_item(&json!({"type":"function_call","call_id":"c","name":"n","arguments":"{\"b\":[2],\"a\":1}"}))
        );
        assert!(canonical_item(&json!({"type":"reasoning","encrypted_content":"x"})).is_none());
        assert_eq!(output_span(&[], &[json!({"type":"function_call_output","call_id":"c","output":"o"})]), Some(0));
        assert_eq!(output_span(&[json!({"type":"message","role":"assistant","content":[]})], &[]), None);
    }
}
