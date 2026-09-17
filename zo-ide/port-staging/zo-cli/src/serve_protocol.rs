//! Wire protocol shared by `zo serve` (the session server) and `zo
//! attach` (the client). One concern: the line-delimited JSON-RPC envelope and
//! the typed method params/results that travel over the TCP socket.
//!
//! ## Framing
//!
//! Every message is a single JSON object on its own line (`\n`-delimited),
//! readable with `nc`/`jq -c` for manual poking. Three message kinds share the
//! one socket, disambiguated **structurally** so no out-of-band tagging is
//! needed:
//!
//! - **Request** (client → server): a [`RpcRequest`] — always carries
//!   `jsonrpc`, `id`, `method`.
//! - **Response** (server → client): an [`RpcResponse`] — always carries
//!   `jsonrpc` + `id`, plus exactly one of `result` / `error`. Terminates a
//!   request.
//! - **Render frame** (server → client, only during `session.run_turn`): the
//!   canonical `SerializableRenderBlock` JSON (carries `type`, **never**
//!   `jsonrpc`). The client streams these until it sees the terminating
//!   response with the matching `id`.
//!
//! Because a render frame never has a `jsonrpc` field and a response always
//! does, the client tells them apart with a single key check — see
//! [`is_response_line`]. The render-frame schema is owned by the `sinks`
//! serializer (`SerializableRenderBlock`); the server reuses that exact
//! serialization (via `NdjsonSink`) so a frame on this socket is byte-identical
//! to a `zo -p --output-format stream-json` line.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Default address the server binds and the client dials. Loopback-only by
/// default — a persistent agent server is not something to expose on `0.0.0.0`
/// without deliberate opt-in.
pub(crate) const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8787";

/// JSON-RPC version string used on every envelope.
pub(crate) const JSONRPC_VERSION: &str = "2.0";

// JSON-RPC error codes (subset of the spec plus one app-specific code).
/// The request envelope is not a valid JSON-RPC 2.0 request.
pub(crate) const CODE_INVALID_REQUEST: i64 = -32600;
/// The method name is not one the server implements.
pub(crate) const CODE_METHOD_NOT_FOUND: i64 = -32601;
/// `params` was missing or did not match the method's expected shape.
pub(crate) const CODE_INVALID_PARAMS: i64 = -32602;
/// Catch-all server-side failure (turn error, persistence failure, …).
pub(crate) const CODE_INTERNAL: i64 = -32603;
/// App-specific: referenced a `session_id` the server has no entry for.
pub(crate) const CODE_NO_SUCH_SESSION: i64 = -32000;
/// App-specific: the turn was cancelled mid-flight via `session.cancel_turn` (F4).
pub(crate) const CODE_CANCELLED: i64 = -32001;
/// App-specific: the request was missing or carried a wrong shared-secret token
/// while the server is running with `ZO_SERVE_TOKEN` set (see
/// [`crate::serve_auth`]).
pub(crate) const CODE_UNAUTHORIZED: i64 = -32002;
/// App-specific (track 5 pair sessions): a `session.steer` was rejected because
/// the connection is not the helm of an in-flight turn (a spectator that only
/// watches, or no turn is running to steer).
pub(crate) const CODE_STEER_DENIED: i64 = -32003;
/// App-specific (track 5 pair sessions): a `session.run_turn` was refused
/// because another turn already holds this session's helm (turn-scoped
/// ownership). The old behavior was to block indefinitely on the session lock;
/// this makes the contention explicit so a second client can decide to spectate
/// instead.
pub(crate) const CODE_HELM_HELD: i64 = -32004;

/// A client → server JSON-RPC request line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RpcRequest {
    pub(crate) jsonrpc: String,
    pub(crate) id: u64,
    pub(crate) method: String,
    /// Method params. Defaults to `null` when the client omits the field.
    #[serde(default)]
    pub(crate) params: JsonValue,
    /// Optional shared-secret token (see [`crate::serve_auth`]). Present only
    /// when the client was configured with `ZO_SERVE_TOKEN`; omitted from the
    /// wire otherwise so tokenless loopback traffic stays unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token: Option<String>,
}

impl RpcRequest {
    /// Build a request envelope with the standard `jsonrpc` version stamped and
    /// no auth token (the tokenless default).
    pub(crate) fn new(id: u64, method: impl Into<String>, params: JsonValue) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.into(),
            params,
            token: None,
        }
    }

    /// Stamp the shared-secret token onto this request (no-op when `token` is
    /// `None`). Used by the clients right before serialization so every request
    /// to a guarded server carries the secret.
    #[must_use]
    pub(crate) fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }
}

/// A server → client JSON-RPC response line. Exactly one of `result` / `error`
/// is `Some` on a well-formed response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RpcResponse {
    pub(crate) jsonrpc: String,
    pub(crate) id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<RpcError>,
}

impl RpcResponse {
    /// Success response carrying `result`.
    pub(crate) fn ok(id: u64, result: JsonValue) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Failure response carrying a typed `error`.
    pub(crate) fn err(id: u64, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// JSON-RPC error body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
}

/// Params for `session.run_turn`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RunTurnParams {
    pub(crate) id: String,
    pub(crate) input: String,
    /// Optional client-chosen id for this turn (F4). When present, the server
    /// registers a cancel hook under it so `session.cancel_turn { turn_id }`
    /// (sent on a second connection) can interrupt the turn mid-flight. Absent
    /// (older clients) → the turn simply cannot be cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) turn_id: Option<u64>,
}

/// Params for `session.run_turn_detached`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RunTurnDetachedParams {
    pub(crate) id: String,
    pub(crate) input: String,
    /// Optional client-chosen id that can be cancelled through
    /// `session.cancel_turn`, same as `session.run_turn`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) turn_id: Option<u64>,
    /// Optional advisory webhook. The server POSTs the terminal job payload
    /// after recording the outcome; delivery failure does not affect the job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) notify_url: Option<String>,
}

/// Params for `session.job_status` and `session.job_result`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct JobIdParams {
    pub(crate) job_id: u64,
}

/// Params for `session.cancel_turn` (F4). `session_id` scopes the cancel to a
/// single session so two sessions that share a `turn_id` never cancel each
/// other. It is optional for backward compatibility: a legacy client that omits
/// it is honoured only when the `turn_id` is unambiguous across sessions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CancelTurnParams {
    pub(crate) turn_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
}

/// Params for `permission.respond` (F2) — the client's decision for a forwarded
/// permission prompt. Sent on a *second* connection (the primary is mid-stream).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PermissionRespondParams {
    /// Echoes the `prompt_id` from the `permission_prompt` render frame so the
    /// server resolves the matching in-flight prompt.
    pub(crate) prompt_id: u64,
    /// Decision tag: `allow_once` | `allow_always` | `deny` | `deny_always`.
    pub(crate) decision: String,
}

/// Params for methods that only need a session id (`session.load`,
/// `session.subscribe`, `session.unsubscribe`, `session.roster`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SessionIdParams {
    pub(crate) id: String,
}

/// Params for `session.subscribe`. `boundary` selects an authoritative
/// turn-boundary snapshot; omitted by legacy clients, it remains a nonblocking
/// cached subscribe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SubscribeParams {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) boundary: bool,
    /// Opt-in for the sticky marker/replacement protocol.  Omitted by legacy
    /// clients, which retain the original non-sticky lag recovery behavior.
    #[serde(default)]
    pub(crate) resync_v2: bool,
}

/// Result of `session.subscribe`. Legacy clients safely ignore the added
/// `subscription_id`; markers use it to make stale deferred delivery harmless.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SubscribeResult {
    pub(crate) id: String,
    pub(crate) history: Vec<HistoryEntry>,
    pub(crate) next_seq: u64,
    pub(crate) helm: Option<String>,
    /// The lowest sequence a v2 client may accept for this replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) floor: Option<u64>,
    /// Missing only identifies a legacy server. A present value must be a u64;
    /// in particular, `null` is not accepted as a second legacy spelling.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_subscription_id"
    )]
    pub(crate) subscription_id: Option<u64>,
}

fn deserialize_subscription_id<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let id = u64::deserialize(deserializer)?;
    if id == 0 {
        return Err(serde::de::Error::custom("subscription_id must be non-zero"));
    }
    Ok(Some(id))
}

/// Params for `session.steer` (track 5 pair sessions) — a mid-turn steering
/// message pushed by the helm on a *second* connection while the primary
/// connection streams the turn. `turn_id`, when present, is validated against
/// the in-flight turn so a stale steer for a finished turn is rejected rather
/// than folded into the next one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SteerParams {
    pub(crate) id: String,
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) turn_id: Option<u64>,
}

/// One persisted conversation message, projected for `session.load` so an
/// attaching client can replay history without speaking the full
/// `ConversationMessage` schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct HistoryEntry {
    /// `"system" | "user" | "assistant" | "tool"`.
    pub(crate) role: String,
    /// Flattened text of the message's content blocks (tool calls/results are
    /// summarized to a single line each).
    pub(crate) text: String,
}

/// One entry in a `session.list` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SessionSummary {
    pub(crate) id: String,
    /// Number of persisted messages in the session.
    pub(crate) messages: usize,
}

/// `session.info` result — the session metadata an attaching TUI client needs
/// to hydrate its sidebar/HUD (model, permission mode, cwd, branch), since it
/// has no local runtime to read these from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SessionInfo {
    pub(crate) id: String,
    /// Active model id, e.g. `claude-opus-4-8`.
    pub(crate) model: String,
    /// Canonical permission-mode label (`read-only` / `workspace-write` /
    /// `danger-full-access`).
    pub(crate) permission_mode: String,
    /// The server's working directory for this session.
    pub(crate) cwd: String,
    /// Current git branch, if the server resolved one.
    pub(crate) git_branch: Option<String>,
}

/// `true` when a server line is a JSON-RPC response (terminates a request)
/// rather than a streamed render frame. A render frame is the canonical
/// `SerializableRenderBlock` JSON, which never carries a `jsonrpc` key.
pub(crate) fn is_response_line(line: &JsonValue) -> bool {
    line.get("jsonrpc").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_result_deserialize_omits_legacy_subscription_id() {
        let result: SubscribeResult = serde_json::from_value(serde_json::json!({
            "id": "session-a",
            "history": [],
            "next_seq": 3,
            "helm": null,
        }))
        .expect("legacy response without subscription id");
        assert_eq!(result.subscription_id, None);
    }

    #[test]
    fn subscribe_result_rejects_malformed_subscription_id() {
        for subscription_id in [serde_json::json!(0), serde_json::json!("wrong"), serde_json::Value::Null] {
            let result = serde_json::from_value::<SubscribeResult>(serde_json::json!({
                "id": "session-a",
                "history": [],
                "next_seq": 3,
                "helm": null,
                "subscription_id": subscription_id,
            }));
            assert!(result.is_err(), "only a missing or u64 id is valid");
        }
    }

    #[test]
    fn request_round_trips_through_json() {
        let req = RpcRequest::new(
            7,
            "session.run_turn",
            serde_json::json!({"id":"s1","input":"hi"}),
        );
        let line = serde_json::to_string(&req).expect("serialize");
        let back: RpcRequest = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(req, back);
        assert_eq!(back.jsonrpc, "2.0");
    }

    #[test]
    fn detached_turn_params_accept_optional_notify_and_turn_id() {
        let params: RunTurnDetachedParams = serde_json::from_value(serde_json::json!({
            "id": "s1",
            "input": "hi",
            "turn_id": 42,
            "notify_url": "http://127.0.0.1:9/hook"
        }))
        .expect("deserialize params");

        assert_eq!(params.id, "s1");
        assert_eq!(params.input, "hi");
        assert_eq!(params.turn_id, Some(42));
        assert_eq!(
            params.notify_url.as_deref(),
            Some("http://127.0.0.1:9/hook")
        );
    }

    #[test]
    fn detached_turn_params_default_optional_fields() {
        let params: RunTurnDetachedParams =
            serde_json::from_value(serde_json::json!({"id": "s1", "input": "hi"}))
                .expect("deserialize params");

        assert_eq!(params.turn_id, None);
        assert_eq!(params.notify_url, None);
    }

    #[test]
    fn job_id_params_deserialize_job_id() {
        let params: JobIdParams =
            serde_json::from_value(serde_json::json!({"job_id": 7})).expect("job id params");
        assert_eq!(params.job_id, 7);
    }

    #[test]
    fn token_is_omitted_from_the_wire_when_absent() {
        let req = RpcRequest::new(1, "session.list", JsonValue::Null);
        let line = serde_json::to_string(&req).expect("serialize");
        assert!(
            !line.contains("token"),
            "a tokenless request must not emit a token field: {line}"
        );
    }

    #[test]
    fn with_token_round_trips_on_the_wire() {
        let req = RpcRequest::new(2, "session.list", JsonValue::Null)
            .with_token(Some("s3cret".to_string()));
        let line = serde_json::to_string(&req).expect("serialize");
        assert!(
            line.contains("\"token\":\"s3cret\""),
            "token must serialize: {line}"
        );
        let back: RpcRequest = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back.token.as_deref(), Some("s3cret"));
    }

    #[test]
    fn request_defaults_missing_params_to_null() {
        let back: RpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"session.list"}"#)
                .expect("deserialize");
        assert_eq!(back.params, JsonValue::Null);
    }

    #[test]
    fn ok_response_omits_error_field() {
        let resp = RpcResponse::ok(3, serde_json::json!({"id":"abc"}));
        let line = serde_json::to_string(&resp).expect("serialize");
        assert!(
            !line.contains("error"),
            "ok response must not serialize error: {line}"
        );
        assert!(line.contains("\"result\""));
    }

    #[test]
    fn err_response_omits_result_field() {
        let resp = RpcResponse::err(4, CODE_NO_SUCH_SESSION, "no such session");
        let line = serde_json::to_string(&resp).expect("serialize");
        assert!(
            !line.contains("result"),
            "err response must not serialize result: {line}"
        );
        assert!(line.contains("\"error\""));
    }

    #[test]
    fn response_line_is_distinguishable_from_render_frame() {
        let response = serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}});
        let render_frame = serde_json::json!({"type":"text_delta","id":1,"text":"hi","done":false});
        assert!(is_response_line(&response));
        assert!(!is_response_line(&render_frame));
    }
}
