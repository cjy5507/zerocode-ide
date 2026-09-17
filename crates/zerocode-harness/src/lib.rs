//! Client for the `zo serve` session server.
//!
//! This is the **structured channel** of the shell IDE. The terminal lane
//! (`zerocode-pty`) hosts a real `zo attach` so a human sees the real TUI;
//! this crate connects to the same server and reports that session as typed
//! frames — session list, turn state, permission prompts — so the IDE chrome
//! never has to screen-scrape the terminal to know what is happening.
//!
//! Two shapes of use, both needed: request/response plus own-turn frames
//! ([`Client::request`]), and passive fan-out from turns other clients drive
//! ([`Client::subscribe`] then [`Client::next_incoming`]).
//!
//! ## The wire, as the server defines it
//!
//! One JSON object per line over TCP. Three message kinds share the socket and
//! are told apart **structurally**:
//!
//! - a request (client → server) always has `jsonrpc`, `id`, `method`;
//! - a response (server → client) always has `jsonrpc` and `id`;
//! - a render frame (server → client, only during a turn) never has `jsonrpc`.
//!
//! So one key decides it: see [`classify`]. This crate mirrors that contract by
//! hand — the server's types are crate-private — which is exactly why the tests
//! pin the discrimination rule and the error codes rather than trusting them.
//!
//! ## Forward compatibility
//!
//! A render frame with an unrecognized `type` is passed through untouched. The
//! client must never fail a turn because the server learned a new frame.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// Where `zo serve` listens unless told otherwise. Loopback, deliberately.
pub const DEFAULT_SERVE_ADDR: &str = "127.0.0.1:8787";

/// Environment variable holding the server's shared secret. When it is set,
/// every request must carry the token or the server answers `-32002`.
pub const TOKEN_ENV: &str = "ZO_SERVE_TOKEN";

/// The prefix of the variables that carry the keys of the routers connected in
/// the window's settings. The window hands them to a zo launch alone; zo
/// adopts them out of its environment at startup, so nothing zo spawns — an
/// MCP server, a tool's shell, a hook — inherits them. One spelling for both
/// sides of that handoff.
pub const ROUTER_KEY_ENV_PREFIX: &str = "ZEROCODE_ROUTER_";

/// Where the window keeps each of those keys: one keychain item per variable,
/// named this prefix and the variable's name. A zo the window did not launch —
/// one typed into a shell pane — reads a key it was not handed from there.
pub const ROUTER_KEYCHAIN_SERVICE_PREFIX: &str = "dev.zerocode.router.";

/// Where the window's settings keep a service key zo reads under the key's own
/// variable name — one keychain item per variable, this prefix and the name.
/// Unlike a router key the window hands nobody this one: every zo, launched by
/// the window or typed into a shell, reads it from the item when first needed.
pub const SERVICE_KEYCHAIN_SERVICE_PREFIX: &str = "dev.zerocode.key.";

/// TypeSafe System One's key variable — the name the TypeSafe SDKs read, and
/// the one zo's System One client reads. The window saves the key under it.
pub const TYPESAFE_API_KEY_ENV: &str = "TYPESAFE_API_KEY";

pub const JSONRPC_VERSION: &str = "2.0";

/// Method names, spelled exactly as the server dispatches them.
pub mod method {
    pub const CREATE: &str = "session.create";
    pub const LIST: &str = "session.list";
    pub const LOAD: &str = "session.load";
    pub const INFO: &str = "session.info";
    pub const CLOSE: &str = "session.close";
    pub const RUN_TURN: &str = "session.run_turn";
    pub const RUN_TURN_DETACHED: &str = "session.run_turn_detached";
    pub const SUBSCRIBE: &str = "session.subscribe";
    pub const UNSUBSCRIBE: &str = "session.unsubscribe";
    pub const STEER: &str = "session.steer";
    pub const ROSTER: &str = "session.roster";
    pub const CANCEL_TURN: &str = "session.cancel_turn";
    pub const PERMISSION_RESPOND: &str = "permission.respond";
    /// The IDE chose a different account; the pane resolves credentials again.
    ///
    /// Its params carry VALUES (`provider`, `label`, `claude_config_dir`,
    /// `codex_home`) because a pane was handed its account through environment
    /// variables at launch and nobody can change a running child's environment
    /// from outside. See `zo-ide/docs/events-channel.md` §3.
    pub const AUTH_RELOAD: &str = "auth.reload";
}

/// Why a call failed, as the server reports it.
///
/// The three app-specific codes carry product meaning, not just failure:
/// `HelmHeld` and `SteerDenied` are the normal outcome of two clients sharing a
/// session, and the UI is expected to offer spectating rather than an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeErrorKind {
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    Internal,
    NoSuchSession,
    Cancelled,
    Unauthorized,
    SteerDenied,
    HelmHeld,
    Unknown(i64),
}

impl ServeErrorKind {
    pub const fn from_code(code: i64) -> Self {
        match code {
            -32600 => ServeErrorKind::InvalidRequest,
            -32601 => ServeErrorKind::MethodNotFound,
            -32602 => ServeErrorKind::InvalidParams,
            -32603 => ServeErrorKind::Internal,
            -32000 => ServeErrorKind::NoSuchSession,
            -32001 => ServeErrorKind::Cancelled,
            -32002 => ServeErrorKind::Unauthorized,
            -32003 => ServeErrorKind::SteerDenied,
            -32004 => ServeErrorKind::HelmHeld,
            other => ServeErrorKind::Unknown(other),
        }
    }

    /// True when another client owns the session. Not a failure to report as
    /// one: the lane should offer to spectate instead.
    pub const fn is_contention(self) -> bool {
        matches!(self, ServeErrorKind::HelmHeld | ServeErrorKind::SteerDenied)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("transport: {0}")]
    Io(#[from] std::io::Error),
    #[error("server closed the connection mid-request")]
    Closed,
    #[error("malformed line from server: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("{method} failed ({code}): {message}")]
    Rpc {
        method: String,
        code: i64,
        kind: ServeErrorKind,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl RpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.into(),
            params,
            token: None,
        }
    }

    #[must_use]
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// One line read from the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<RpcError>,
    },
    /// A streaming render block. Held as raw JSON: the schema belongs to the
    /// server's renderer and grows over time.
    Frame(Value),
}

/// Decide what a line is. The rule is one key: a response always carries
/// `jsonrpc`, a render frame never does.
///
/// # Errors
///
/// [`HarnessError::Malformed`] if the line is not JSON at all. A line that *is*
/// JSON but has a shape we do not recognise is a [`Incoming::Frame`], not an
/// error — the renderer's schema grows, and refusing what we have not seen
/// would break on every server release.
pub fn classify(line: &str) -> Result<Incoming, HarnessError> {
    let value: Value = serde_json::from_str(line)?;
    if value.get("jsonrpc").is_none() {
        return Ok(Incoming::Frame(value));
    }
    let id = value.get("id").and_then(Value::as_u64).unwrap_or_default();
    let error = value
        .get("error")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?;
    Ok(Incoming::Response {
        id,
        result: value.get("result").cloned(),
        error,
    })
}

/// The first thing a person asked, dug out of a `session.subscribe` history.
///
/// The shape the server actually sends is `{ "role", "text" }` per entry, with
/// `role` one of `system | user | assistant | tool` — read from its own
/// `HistoryEntry`, which is crate-private to it. Since we cannot import that
/// type, this reads **tolerantly** rather than deserializing into a mirror of
/// it: the same stance the frame handling takes, and for the same reason. A
/// shape we have not seen must degrade to "no name yet", never to an error.
///
/// Filtering on the role is what makes it right rather than merely working. A
/// transcript usually opens with the system prompt, so taking entry zero would
/// name every session after the prompt they all share.
#[must_use]
pub fn first_question_in(history: &Value) -> Option<String> {
    history.as_array()?.iter().find_map(question_in_entry)
}

fn question_in_entry(entry: &Value) -> Option<String> {
    if let Some(text) = entry.as_str() {
        return non_empty(text);
    }
    let object = entry.as_object()?;
    // Only when the entry names a role at all: an unlabelled history is still
    // usable, and refusing it would leave every session showing its id.
    if let Some(role) = object.get("role").and_then(Value::as_str)
        && role != "user"
    {
        return None;
    }
    ["text", "content", "input", "message", "prompt"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .and_then(non_empty)
}

fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// A connected session server.
pub struct Client {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    token: Option<String>,
    next_id: u64,
}

impl Client {
    /// Connect and remember the token. Reads `ZO_SERVE_TOKEN` when `token` is
    /// `None`, so a guarded server works without extra wiring in the shell.
    ///
    /// Connecting proves nothing about who answered — any process can hold a
    /// loopback port. Callers that are about to hand over the shared secret
    /// should identify the listener first; `zerocode-lane`'s two-step handshake
    /// is what that looks like.
    ///
    /// # Errors
    ///
    /// [`HarnessError::Io`] if the address cannot be reached.
    pub async fn connect(addr: &str, token: Option<String>) -> Result<Self, HarnessError> {
        let stream = TcpStream::connect(addr).await?;
        let (read, write) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read),
            writer: write,
            token: token.or_else(|| std::env::var(TOKEN_ENV).ok()),
            next_id: 1,
        })
    }

    /// Send a request and drive the connection until its response arrives,
    /// handing every render frame seen on the way to `on_frame`.
    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        on_frame: &mut dyn FnMut(Value),
    ) -> Result<Value, HarnessError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = RpcRequest::new(id, method, params).with_token(self.token.clone());
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;

        let mut buffer = String::new();
        loop {
            buffer.clear();
            if self.reader.read_line(&mut buffer).await? == 0 {
                return Err(HarnessError::Closed);
            }
            let trimmed = buffer.trim();
            if trimmed.is_empty() {
                continue;
            }
            match classify(trimmed)? {
                Incoming::Frame(frame) => on_frame(frame),
                Incoming::Response {
                    id: response_id,
                    result,
                    error,
                } => {
                    if response_id != id {
                        continue;
                    }
                    if let Some(error) = error {
                        return Err(HarnessError::Rpc {
                            method: method.to_string(),
                            code: error.code,
                            kind: ServeErrorKind::from_code(error.code),
                            message: error.message,
                        });
                    }
                    return Ok(result.unwrap_or(Value::Null));
                }
            }
        }
    }

    /// Convenience for a request whose frames are not interesting.
    ///
    /// # Errors
    ///
    /// Whatever [`Client::request`] returns: [`HarnessError::Rpc`] when the
    /// server refuses — check its [`ServeErrorKind`], since a held helm is
    /// contention rather than failure — or [`HarnessError::Io`] /
    /// [`HarnessError::Closed`] if the connection breaks first.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, HarnessError> {
        self.request(method, params, &mut |_| {}).await
    }

    /// Join a session's fan-out so this connection receives frames from turns
    /// **other clients** are driving — including the `zo attach` running in our
    /// own terminal lane.
    ///
    /// `boundary` asks the server for an atomic `{history, next_seq}` snapshot
    /// as the hydration base, which is what makes re-subscribing after a
    /// reconnect safe instead of a source of duplicated history.
    pub async fn subscribe(
        &mut self,
        session_id: &str,
        boundary: bool,
    ) -> Result<Value, HarnessError> {
        self.call(
            method::SUBSCRIBE,
            json!({ "id": session_id, "boundary": boundary }),
        )
        .await
    }

    /// Leave a session's fan-out. Frames from other clients stop arriving; the
    /// session itself is untouched.
    ///
    /// # Errors
    ///
    /// [`HarnessError::Rpc`] if the server refuses, or [`HarnessError::Io`] if
    /// the connection is already gone.
    pub async fn unsubscribe(&mut self, session_id: &str) -> Result<Value, HarnessError> {
        self.call(method::UNSUBSCRIBE, json!({ "id": session_id }))
            .await
    }

    /// Subscribe and report what the session should be **called** — the first
    /// thing asked in it, or `None` when nobody has asked anything yet.
    ///
    /// Uses the hydration snapshot rather than a separate call, so naming a
    /// session costs nothing beyond the subscribe a lane needs anyway.
    ///
    /// # Errors
    ///
    /// Whatever [`Client::subscribe`] returns. A history whose shape we do not
    /// recognise is `Ok(None)`, not an error — see [`first_question_in`].
    pub async fn session_name(&mut self, session_id: &str) -> Result<Option<String>, HarnessError> {
        let hydrated = self.subscribe(session_id, true).await?;
        Ok(hydrated.get("history").and_then(first_question_in))
    }

    /// Read the next line the server pushes with no request outstanding.
    ///
    /// After [`Client::subscribe`] the socket stops being request/response —
    /// frames arrive whenever another client drives a turn. `None` means the
    /// server closed, which the caller should treat as "reconnect and
    /// re-subscribe", not as a failure.
    ///
    /// # Errors
    ///
    /// [`HarnessError::Io`] if the read fails, or [`HarnessError::Malformed`] if
    /// the line is not JSON. An orderly close is `Ok(None)` instead.
    pub async fn next_incoming(&mut self) -> Result<Option<Incoming>, HarnessError> {
        let mut buffer = String::new();
        loop {
            buffer.clear();
            if self.reader.read_line(&mut buffer).await? == 0 {
                return Ok(None);
            }
            let trimmed = buffer.trim();
            if trimmed.is_empty() {
                continue;
            }
            return classify(trimmed).map(Some);
        }
    }

    /// Run one turn, streaming its render frames.
    pub async fn run_turn(
        &mut self,
        session_id: &str,
        input: &str,
        turn_id: Option<u64>,
        on_frame: &mut dyn FnMut(Value),
    ) -> Result<Value, HarnessError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), json!(session_id));
        params.insert("input".to_string(), json!(input));
        if let Some(turn_id) = turn_id {
            params.insert("turn_id".to_string(), json!(turn_id));
        }
        self.request(method::RUN_TURN, json!(params), on_frame)
            .await
    }

    /// Answer a permission prompt. `decision` is one of `allow_once`,
    /// `allow_always`, `deny`, `deny_always`.
    pub async fn respond_to_permission(
        &mut self,
        prompt_id: u64,
        decision: &str,
    ) -> Result<Value, HarnessError> {
        self.call(
            method::PERMISSION_RESPOND,
            json!({ "prompt_id": prompt_id, "decision": decision }),
        )
        .await
    }

    /// Inject text into an in-flight turn.
    pub async fn steer(
        &mut self,
        session_id: &str,
        text: &str,
        turn_id: Option<u64>,
    ) -> Result<Value, HarnessError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), json!(session_id));
        params.insert("text".to_string(), json!(text));
        if let Some(turn_id) = turn_id {
            params.insert("turn_id".to_string(), json!(turn_id));
        }
        self.call(method::STEER, json!(params)).await
    }
}
