//! LSP (Language Server Protocol) client registry for tool dispatch.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::lock::Mutex as AsyncMutex;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::mcp_client::McpStdioTransport;
use crate::mcp_stdio::{JsonRpcResponse, McpStdioProcess};

/// Sink invoked by the reader task whenever the language server publishes
/// diagnostics for a document. Stored behind an `Option` so it can be
/// attached after the reader task is already running (the registry injects
/// it during `register_with_transport`).
type DiagnosticsSink = Box<dyn Fn(Vec<LspDiagnostic>) + Send + Sync>;

#[cfg(test)]
use std::future::pending;

#[cfg(test)]
const LSP_DISPATCH_TIMEOUT_MS: u64 = 5_000;
#[cfg(not(test))]
const LSP_DISPATCH_TIMEOUT_MS: u64 = 10_000;

#[cfg(test)]
const MAX_OPENED_DOCUMENTS_PER_TRANSPORT: usize = 4;
#[cfg(not(test))]
const MAX_OPENED_DOCUMENTS_PER_TRANSPORT: usize = 128;

#[cfg(test)]
const MAX_DIAGNOSTICS_PER_SERVER: usize = 8;
#[cfg(not(test))]
const MAX_DIAGNOSTICS_PER_SERVER: usize = 1_000;

/// Supported LSP actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LspAction {
    Diagnostics,
    Hover,
    Definition,
    References,
    Completion,
    Symbols,
    Format,
}

impl LspAction {
    #[must_use]
    pub fn parse_action(s: &str) -> Option<Self> {
        match s {
            "diagnostics" => Some(Self::Diagnostics),
            "hover" => Some(Self::Hover),
            "definition" | "goto_definition" => Some(Self::Definition),
            "references" | "find_references" => Some(Self::References),
            "completion" | "completions" => Some(Self::Completion),
            "symbols" | "document_symbols" => Some(Self::Symbols),
            "format" | "formatting" => Some(Self::Format),
            _ => None,
        }
    }

    #[must_use]
    pub fn capability_name(self) -> &'static str {
        match self {
            Self::Diagnostics => "diagnostics",
            Self::Hover => "hover",
            Self::Definition => "definition",
            Self::References => "references",
            Self::Completion => "completion",
            Self::Symbols => "symbols",
            Self::Format => "format",
        }
    }
}

pub trait LspTransport: Send + Sync + std::fmt::Debug {
    fn dispatch(
        &self,
        action: LspAction,
        path: &str,
        line: u32,
        character: u32,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<JsonValue, String>> + Send>>;

    /// Attach a callback that receives diagnostics pushed by the language
    /// server via `textDocument/publishDiagnostics`. The default
    /// implementation is a no-op so non-stdio / mock transports remain
    /// unaffected; only [`LspStdioTransport`] wires this into its reader task.
    fn attach_diagnostics_sink(&self, _sink: Box<dyn Fn(Vec<LspDiagnostic>) + Send + Sync>) {}

    /// Notify the language server that a document's full text changed.
    /// Default no-op keeps mock and non-stdio transports unaffected.
    fn notify_document_changed(&self, _path: &str, _text: &str) {}
}

/// Channel used by the reader task to hand a JSON-RPC response body back to
/// the originating `dispatch_request`/`initialize` caller.
type PendingResponses = Arc<AsyncMutex<HashMap<u64, oneshot::Sender<JsonValue>>>>;

#[derive(Clone)]
pub struct LspStdioTransport {
    label: Arc<str>,
    /// Writer half of the child's stdin, shared so concurrent dispatches can
    /// serialize their frames.
    stdin: Arc<AsyncMutex<ChildStdin>>,
    /// Child handle, retained for termination. The reader task owns stdout.
    child: Arc<AsyncMutex<Child>>,
    /// Outstanding request ids awaiting a response, keyed by numeric id.
    pending: PendingResponses,
    /// Diagnostics callback injected by the registry after spawn.
    diagnostics_sink: Arc<Mutex<Option<DiagnosticsSink>>>,
    /// Documents already announced via `textDocument/didOpen`.
    opened: Arc<Mutex<HashSet<String>>>,
    /// FIFO order for opened document URIs, used to evict old `didOpen` state.
    opened_order: Arc<Mutex<VecDeque<String>>>,
    /// Last version sent for each opened document uri.
    versions: Arc<Mutex<HashMap<String, i64>>>,
    next_id: Arc<AtomicU64>,
    runtime_handle: Option<tokio::runtime::Handle>,
}

enum OpenDocumentReservation {
    AlreadyOpen,
    Reserved { evicted_uri: Option<String> },
}

impl std::fmt::Debug for LspStdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspStdioTransport")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl LspStdioTransport {
    pub fn spawn(transport: &McpStdioTransport) -> std::io::Result<Self> {
        let label: Arc<str> = transport.command.clone().into();
        let process = McpStdioProcess::spawn(transport)?;
        let (child, stdin, stdout) = process.into_parts();

        let pending: PendingResponses = Arc::new(AsyncMutex::new(HashMap::new()));
        let diagnostics_sink: Arc<Mutex<Option<DiagnosticsSink>>> = Arc::new(Mutex::new(None));

        // Background reader: continuously drains LSP frames, routing
        // id-bearing responses to their pending sender and
        // publishDiagnostics notifications to the diagnostics sink.
        tokio::spawn(reader_loop(
            stdout,
            Arc::clone(&pending),
            Arc::clone(&diagnostics_sink),
        ));

        Ok(Self {
            label,
            stdin: Arc::new(AsyncMutex::new(stdin)),
            child: Arc::new(AsyncMutex::new(child)),
            pending,
            diagnostics_sink,
            opened: Arc::new(Mutex::new(HashSet::new())),
            opened_order: Arc::new(Mutex::new(VecDeque::new())),
            versions: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            runtime_handle: None,
        })
    }

    pub async fn spawn_initialized(
        transport: McpStdioTransport,
        root_path: Option<&str>,
    ) -> Result<Self, String> {
        let mut spawned = Self::spawn(&transport).map_err(|error| error.to_string())?;
        spawned.runtime_handle = Some(tokio::runtime::Handle::current());
        spawned.initialize(root_path).await?;
        Ok(spawned)
    }

    fn next_numeric_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn method_for_action(action: LspAction) -> &'static str {
        match action {
            LspAction::Diagnostics => "textDocument/publishDiagnostics",
            LspAction::Hover => "textDocument/hover",
            LspAction::Definition => "textDocument/definition",
            LspAction::References => "textDocument/references",
            LspAction::Completion => "textDocument/completion",
            LspAction::Symbols => "textDocument/documentSymbol",
            LspAction::Format => "textDocument/formatting",
        }
    }

    fn path_to_uri(path: &str) -> String {
        let resolved =
            std::env::current_dir().map_or_else(|_| PathBuf::from(path), |cwd| cwd.join(path));
        let canonical = resolved.canonicalize().unwrap_or(resolved);
        format!("file://{}", canonical.to_string_lossy())
    }

    /// Map a file extension to the LSP `languageId` used in `didOpen`.
    fn language_id_for_path(path: &str) -> &'static str {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        match ext {
            "rs" => "rust",
            "ts" => "typescript",
            "tsx" => "typescriptreact",
            "js" => "javascript",
            "jsx" => "javascriptreact",
            "py" => "python",
            "go" => "go",
            "java" => "java",
            "c" | "h" => "c",
            "cpp" | "hpp" | "cc" => "cpp",
            "rb" => "ruby",
            "lua" => "lua",
            _ => "plaintext",
        }
    }

    fn params_for_action(action: LspAction, path: &str, line: u32, character: u32) -> JsonValue {
        let text_document = serde_json::json!({ "uri": Self::path_to_uri(path) });
        let position = serde_json::json!({
            "line": line,
            "character": character,
        });

        match action {
            LspAction::Diagnostics | LspAction::Symbols => serde_json::json!({
                "textDocument": text_document,
            }),
            LspAction::Hover | LspAction::Definition | LspAction::Completion => serde_json::json!({
                "textDocument": text_document,
                "position": position,
            }),
            LspAction::References => serde_json::json!({
                "textDocument": text_document,
                "position": position,
                "context": {
                    "includeDeclaration": true
                }
            }),
            LspAction::Format => serde_json::json!({
                "textDocument": text_document,
                "options": {
                    "tabSize": 4,
                    "insertSpaces": true
                }
            }),
        }
    }

    /// Send a JSON-RPC request frame and await its response via the reader
    /// task. Registers a oneshot in `pending` keyed by the numeric id before
    /// writing, then waits up to [`LSP_DISPATCH_TIMEOUT_MS`].
    async fn send_request(&self, method: &str, params: JsonValue) -> Result<JsonValue, String> {
        let id = self.next_numeric_id();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_message(&request).await {
            self.pending.lock().await.remove(&id);
            return Err(format!(
                "LSP transport request failed for {method}: {error}"
            ));
        }

        let body = match timeout(Duration::from_millis(LSP_DISPATCH_TIMEOUT_MS), rx).await {
            Ok(Ok(body)) => body,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                return Err(format!(
                    "LSP transport closed before responding to {method}"
                ));
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(format!(
                    "LSP request {method} timed out after {LSP_DISPATCH_TIMEOUT_MS} ms"
                ));
            }
        };

        let response: JsonRpcResponse<JsonValue> = serde_json::from_value(body)
            .map_err(|error| format!("LSP response for {method} was malformed: {error}"))?;
        if let Some(error) = response.error {
            return Err(format!(
                "LSP server returned {method} error {}: {}",
                error.code, error.message
            ));
        }
        response
            .result
            .ok_or_else(|| format!("LSP server returned no result for {method}"))
    }

    /// Send a JSON-RPC notification (no `id`, no response expected).
    async fn send_notification(&self, method: &str, params: JsonValue) -> Result<(), String> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&notification)
            .await
            .map_err(|error| format!("LSP notification {method} failed: {error}"))
    }

    /// Serialize and frame an outgoing JSON-RPC message onto stdin.
    async fn write_message(&self, message: &JsonValue) -> io::Result<()> {
        let body = serde_json::to_vec(message)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let framed = encode_lsp_frame(&body);
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&framed).await?;
        stdin.flush().await
    }

    fn reserve_document_uri(&self, uri: &str) -> OpenDocumentReservation {
        {
            let opened = self.opened.lock().expect("lsp opened set poisoned");
            if opened.contains(uri) {
                return OpenDocumentReservation::AlreadyOpen;
            }
        }

        let mut evicted_uri = None;
        {
            let mut opened = self.opened.lock().expect("lsp opened set poisoned");
            let mut versions = self.versions.lock().expect("lsp versions map poisoned");
            let mut order = self.opened_order.lock().expect("lsp opened order poisoned");

            if !opened.insert(uri.to_string()) {
                return OpenDocumentReservation::AlreadyOpen;
            }
            versions.entry(uri.to_string()).or_insert(1);
            order.push_back(uri.to_string());

            while opened.len() > MAX_OPENED_DOCUMENTS_PER_TRANSPORT {
                let Some(candidate) = order.pop_front() else {
                    break;
                };
                if candidate == uri {
                    order.push_back(candidate);
                    break;
                }
                if opened.remove(&candidate) {
                    versions.remove(&candidate);
                    evicted_uri = Some(candidate);
                    break;
                }
            }
        }

        OpenDocumentReservation::Reserved { evicted_uri }
    }

    fn forget_document_uri(&self, uri: &str) {
        self.opened
            .lock()
            .expect("lsp opened set poisoned")
            .remove(uri);
        self.versions
            .lock()
            .expect("lsp versions map poisoned")
            .remove(uri);
        self.opened_order
            .lock()
            .expect("lsp opened order poisoned")
            .retain(|entry| entry != uri);
    }

    #[cfg(test)]
    fn opened_document_count(&self) -> usize {
        self.opened.lock().expect("lsp opened set poisoned").len()
    }

    /// Lazily open `path` via `textDocument/didOpen` so the server begins
    /// publishing diagnostics for it. Reads file contents once; subsequent
    /// dispatches for the same path are no-ops until the per-transport FIFO cap
    /// evicts the document and sends a best-effort `didClose`.
    async fn ensure_document_open(&self, path: &str) -> Result<(), String> {
        let uri = Self::path_to_uri(path);
        {
            let opened = self.opened.lock().expect("lsp opened set poisoned");
            if opened.contains(&uri) {
                return Ok(());
            }
        }

        let language_id = Self::language_id_for_path(path);
        let resolved =
            std::env::current_dir().map_or_else(|_| PathBuf::from(path), |cwd| cwd.join(path));
        let text = tokio::fs::read_to_string(&resolved)
            .await
            .unwrap_or_default();

        let evicted_uri = match self.reserve_document_uri(&uri) {
            OpenDocumentReservation::AlreadyOpen => return Ok(()),
            OpenDocumentReservation::Reserved { evicted_uri } => evicted_uri,
        };

        if let Some(evicted_uri) = evicted_uri {
            let _ = self
                .send_notification(
                    "textDocument/didClose",
                    serde_json::json!({
                        "textDocument": {
                            "uri": evicted_uri,
                        }
                    }),
                )
                .await;
        }

        let opened = self
            .send_notification(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": language_id,
                        "version": 1,
                        "text": text,
                    }
                }),
            )
            .await;
        if opened.is_err() {
            self.forget_document_uri(&uri);
        }
        opened
    }

    async fn dispatch_request(
        &self,
        action: LspAction,
        path: &str,
        line: u32,
        character: u32,
    ) -> Result<JsonValue, String> {
        // Make sure the server has the document open so it can answer
        // position-based queries and publish diagnostics for it.
        self.ensure_document_open(path).await?;
        let method = Self::method_for_action(action);
        let params = Self::params_for_action(action, path, line, character);
        self.send_request(method, params).await
    }

    async fn initialize(&self, root_path: Option<&str>) -> Result<(), String> {
        let root_uri = root_path.map_or_else(|| Self::path_to_uri("."), Self::path_to_uri);
        let params = serde_json::json!({
            "processId": std::process::id(),
            "clientInfo": {
                "name": "zo",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "rootUri": root_uri,
            "capabilities": {}
        });
        self.send_request("initialize", params)
            .await
            .map_err(|error| format!("LSP initialize failed: {error}"))?;
        // The server only starts processing further requests / publishing
        // diagnostics once it receives the `initialized` acknowledgement.
        self.send_notification("initialized", serde_json::json!({}))
            .await
    }

    pub async fn terminate(&self) -> Result<(), String> {
        let mut child = self.child.lock().await;
        // Send SIGKILL but do NOT await the reap. `Child::kill().await` is
        // `start_kill()` + `wait().await`, and that `wait()` needs the spawning
        // runtime's signal driver to observe SIGCHLD. During a live `/permission`
        // or `/model` rebuild, `shutdown` runs via
        // `block_in_place(|| owned_runtime.block_on(terminate()))` — driving the
        // future from the parked TUI worker, where the single-worker owned
        // runtime's signal driver never makes progress, so `wait()` hangs forever
        // (the Shift+Tab freeze). `start_kill` is synchronous: it sends the kill
        // and returns, so `terminate` resolves in one poll (the `child` lock is
        // uncontended — only this method locks it; the reader loop owns `stdout`).
        // The zombie is reaped by the owned runtime's process driver on the next
        // SIGCHLD, or by the OS at process exit — a brief zombie across a rare
        // rebuild is harmless.
        child
            .start_kill()
            .map_err(|error| format!("failed to terminate LSP stdio transport: {error}"))
    }
}

impl LspTransport for LspStdioTransport {
    fn dispatch(
        &self,
        action: LspAction,
        path: &str,
        line: u32,
        character: u32,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<JsonValue, String>> + Send>> {
        let transport = self.clone();
        let path = path.to_string();
        if let Some(handle) = self.runtime_handle.clone() {
            let label = transport.label.clone();
            Box::pin(async move {
                handle
                    .spawn(async move {
                        transport
                            .dispatch_request(action, &path, line, character)
                            .await
                    })
                    .await
                    .map_err(|error| format!("LSP runtime task failed for {label}: {error}"))?
            })
        } else {
            Box::pin(async move {
                transport
                    .dispatch_request(action, &path, line, character)
                    .await
            })
        }
    }

    fn attach_diagnostics_sink(&self, sink: Box<dyn Fn(Vec<LspDiagnostic>) + Send + Sync>) {
        let mut slot = self
            .diagnostics_sink
            .lock()
            .expect("lsp diagnostics sink poisoned");
        *slot = Some(sink);
    }

    fn notify_document_changed(&self, path: &str, text: &str) {
        let Some(handle) = self.runtime_handle.clone() else {
            return;
        };

        let transport = self.clone();
        let path = path.to_owned();
        let text = text.to_owned();
        handle.spawn(async move {
            if transport.ensure_document_open(&path).await.is_err() {
                return;
            }
            let uri = LspStdioTransport::path_to_uri(&path);
            let version = {
                let mut versions = transport
                    .versions
                    .lock()
                    .expect("lsp versions map poisoned");
                let next = versions.get(&uri).copied().unwrap_or(1) + 1;
                versions.insert(uri.clone(), next);
                next
            };
            let _ = transport
                .send_notification(
                    "textDocument/didChange",
                    serde_json::json!({
                        "textDocument": {
                            "uri": uri,
                            "version": version,
                        },
                        "contentChanges": [
                            { "text": text }
                        ]
                    }),
                )
                .await;
        });
    }
}

/// Background reader: drains LSP frames from `stdout` for the process
/// lifetime. Id-bearing JSON-RPC responses are routed to the matching
/// pending oneshot; `textDocument/publishDiagnostics` notifications are
/// converted and forwarded to the diagnostics sink; everything else
/// (window/logMessage, progress, etc.) is ignored. Exits when the pipe
/// closes.
async fn reader_loop(
    mut reader: BufReader<ChildStdout>,
    pending: PendingResponses,
    diagnostics_sink: Arc<Mutex<Option<DiagnosticsSink>>>,
) {
    loop {
        // EOF or a malformed frame ends the reader; outstanding callers
        // observe a closed channel and surface a transport error.
        let Ok(payload) = read_lsp_frame(&mut reader).await else {
            break;
        };

        let Ok(message) = serde_json::from_slice::<JsonValue>(&payload) else {
            continue;
        };

        if let Some(id) = message.get("id").and_then(JsonValue::as_u64) {
            // Response to one of our requests.
            if let Some(sender) = pending.lock().await.remove(&id) {
                let _ = sender.send(message);
            }
            continue;
        }

        // Notification (no id). Only publishDiagnostics is actionable.
        if message.get("method").and_then(JsonValue::as_str)
            == Some("textDocument/publishDiagnostics")
        {
            if let Some(params) = message.get("params") {
                let diagnostics = parse_publish_diagnostics(params);
                if !diagnostics.is_empty() || params.get("uri").is_some() {
                    if let Ok(slot) = diagnostics_sink.lock() {
                        if let Some(sink) = slot.as_ref() {
                            sink(diagnostics);
                        }
                    }
                }
            }
        }
    }
}

/// Convert a `textDocument/publishDiagnostics` params object into the
/// registry's [`LspDiagnostic`] shape. The `uri` is reduced back to a file
/// path (inverse of [`LspStdioTransport::path_to_uri`]).
fn parse_publish_diagnostics(params: &JsonValue) -> Vec<LspDiagnostic> {
    let path = params
        .get("uri")
        .and_then(JsonValue::as_str)
        .map(uri_to_path)
        .unwrap_or_default();
    let Some(items) = params.get("diagnostics").and_then(JsonValue::as_array) else {
        return Vec::new();
    };

    items
        .iter()
        .map(|item| {
            let start = item.get("range").and_then(|range| range.get("start"));
            let line = u32::try_from(
                start
                    .and_then(|s| s.get("line"))
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0),
            )
            .unwrap_or(u32::MAX);
            let character = u32::try_from(
                start
                    .and_then(|s| s.get("character"))
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0),
            )
            .unwrap_or(u32::MAX);
            let severity = severity_label(item.get("severity").and_then(JsonValue::as_u64));
            let message = item
                .get("message")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string();
            let source = item
                .get("source")
                .and_then(JsonValue::as_str)
                .map(str::to_string);
            LspDiagnostic {
                path: path.clone(),
                line,
                character,
                severity,
                message,
                source,
            }
        })
        .collect()
}

/// Map an LSP `DiagnosticSeverity` integer to a human-readable label.
/// Unknown / missing severities default to `error`, matching the LSP
/// convention that an absent severity is treated as the most severe.
fn severity_label(severity: Option<u64>) -> String {
    match severity {
        Some(2) => "warning",
        Some(3) => "info",
        Some(4) => "hint",
        _ => "error",
    }
    .to_string()
}

/// Convert a `file://` URI back into the path representation callers use to
/// query diagnostics. `path_to_uri` canonicalizes against the cwd, so the
/// inverse re-relativizes against the cwd when the document lives inside the
/// workspace; otherwise the absolute path is kept. This keeps cache keys in
/// step with the relative paths supplied to `get_diagnostics` / the
/// `diagnostics` dispatch action.
fn uri_to_path(uri: &str) -> String {
    let absolute = uri.strip_prefix("file://").unwrap_or(uri);
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(canonical_cwd) = cwd.canonicalize() {
            if let Ok(relative) = std::path::Path::new(absolute).strip_prefix(&canonical_cwd) {
                return relative.to_string_lossy().into_owned();
            }
        }
        if let Ok(relative) = std::path::Path::new(absolute).strip_prefix(&cwd) {
            return relative.to_string_lossy().into_owned();
        }
    }
    absolute.to_string()
}

/// Reshape a language server's raw JSON-RPC result into the model-facing
/// structs for the action that produced it. Hover/definition/references/
/// completion/symbols carry `file://` URIs and 0-indexed positions; the
/// normalized form uses workspace-relative paths (via [`uri_to_path`]),
/// 1-indexed positions, and a short source preview where available.
///
/// Any result that does not match the recognized LSP shape (for example a
/// mock transport returning an arbitrary object) is returned unchanged so
/// callers and tests that pass synthetic payloads keep working.
fn normalize_dispatch_result(action: LspAction, raw: JsonValue) -> JsonValue {
    match action {
        LspAction::Hover => normalize_hover(&raw).map_or(raw, into_json),
        LspAction::Definition | LspAction::References => {
            normalize_locations(&raw).map_or(raw, into_json)
        }
        LspAction::Completion => normalize_completion(&raw).map_or(raw, into_json),
        LspAction::Symbols => normalize_symbols(&raw).map_or(raw, into_json),
        LspAction::Diagnostics | LspAction::Format => raw,
    }
}

fn into_json<T: Serialize>(value: T) -> JsonValue {
    serde_json::to_value(value).unwrap_or(JsonValue::Null)
}

/// Convert a 0-indexed LSP line/character into a 1-indexed position. LSP uses
/// 0-based lines and columns; editors and the model think 1-based.
fn one_indexed(value: &JsonValue, key: &str) -> u32 {
    value
        .get(key)
        .and_then(JsonValue::as_u64)
        .and_then(|n| u32::try_from(n.saturating_add(1)).ok())
        .unwrap_or(1)
}

fn one_indexed_opt(value: &JsonValue, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(JsonValue::as_u64)
        .and_then(|n| u32::try_from(n.saturating_add(1)).ok())
}

/// Extract human-readable text from the three `Hover.contents` shapes the
/// protocol allows: a bare `MarkedString` (`String` | `{language, value}`),
/// a `MarkupContent` (`{kind, value}`), or an array of `MarkedString`s.
/// Returns `None` when no textual content is present so the caller can fall
/// back to passing the raw payload through.
fn normalize_hover(raw: &JsonValue) -> Option<LspHoverResult> {
    let contents = raw.get("contents")?;
    match contents {
        JsonValue::String(text) => Some(LspHoverResult {
            content: text.clone(),
            language: None,
        }),
        JsonValue::Object(map) => {
            // MarkupContent ({kind, value}) or MarkedString ({language, value}):
            // a MarkedString carries a syntax `language`, a MarkupContent carries
            // a markup `kind` (markdown/plaintext) — surface whichever is present.
            let content = map.get("value").and_then(JsonValue::as_str)?.to_string();
            let language = map
                .get("language")
                .or_else(|| map.get("kind"))
                .and_then(JsonValue::as_str)
                .map(str::to_string);
            Some(LspHoverResult { content, language })
        }
        JsonValue::Array(items) => {
            let mut parts = Vec::new();
            let mut language = None;
            for item in items {
                match item {
                    JsonValue::String(text) => parts.push(text.clone()),
                    JsonValue::Object(map) => {
                        if let Some(value) = map.get("value").and_then(JsonValue::as_str) {
                            parts.push(value.to_string());
                        }
                        if language.is_none() {
                            language = map
                                .get("language")
                                .and_then(JsonValue::as_str)
                                .map(str::to_string);
                        }
                    }
                    _ => {}
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(LspHoverResult {
                    content: parts.join("\n"),
                    language,
                })
            }
        }
        _ => None,
    }
}

/// Normalize a `definition`/`references` result — a single `Location`, an
/// array of `Location`s, or an array of `LocationLink`s — into relative-path,
/// 1-indexed [`LspLocation`]s with a source preview line. Returns `None` when
/// no entry carries a `uri`/`targetUri`, so synthetic payloads pass through.
fn normalize_locations(raw: &JsonValue) -> Option<Vec<LspLocation>> {
    let entries: Vec<&JsonValue> = match raw {
        JsonValue::Array(items) => items.iter().collect(),
        JsonValue::Object(_) => vec![raw],
        _ => return None,
    };

    let mut locations = Vec::new();
    for entry in entries {
        // `Location` uses `uri`/`range`; `LocationLink` uses
        // `targetUri`/`targetSelectionRange` (falling back to `targetRange`).
        let uri = entry
            .get("uri")
            .or_else(|| entry.get("targetUri"))
            .and_then(JsonValue::as_str);
        let Some(uri) = uri else { continue };
        let range = entry
            .get("range")
            .or_else(|| entry.get("targetSelectionRange"))
            .or_else(|| entry.get("targetRange"));
        let start = range.and_then(|r| r.get("start"));
        let end = range.and_then(|r| r.get("end"));

        let path = uri_to_path(uri);
        let line = start.map_or(1, |s| one_indexed(s, "line"));
        let character = start.map_or(1, |s| one_indexed(s, "character"));
        let end_line = end.and_then(|e| one_indexed_opt(e, "line"));
        let end_character = end.and_then(|e| one_indexed_opt(e, "character"));
        let preview = preview_line(&path, line);

        locations.push(LspLocation {
            path,
            line,
            character,
            end_line,
            end_character,
            preview,
        });
    }

    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

/// Normalize a `documentSymbol` result. Servers return either a hierarchical
/// `DocumentSymbol[]` (with `range`/`selectionRange`/`children`) or a flat
/// `SymbolInformation[]` (with `location.uri`/`location.range`). Both are
/// flattened into [`LspSymbol`]s with relative paths and 1-indexed positions.
fn normalize_symbols(raw: &JsonValue) -> Option<Vec<LspSymbol>> {
    let items = raw.as_array()?;
    if items.is_empty() {
        return None;
    }
    let mut symbols = Vec::new();
    for item in items {
        collect_symbol(item, &mut symbols);
    }
    if symbols.is_empty() {
        None
    } else {
        Some(symbols)
    }
}

fn collect_symbol(item: &JsonValue, out: &mut Vec<LspSymbol>) {
    let Some(name) = item.get("name").and_then(JsonValue::as_str) else {
        return;
    };
    let kind = symbol_kind_label(item.get("kind").and_then(JsonValue::as_u64));

    // `SymbolInformation` nests position under `location`; `DocumentSymbol`
    // carries `selectionRange`/`range` directly and may have `children`.
    if let Some(location) = item.get("location") {
        let path = location
            .get("uri")
            .and_then(JsonValue::as_str)
            .map(uri_to_path)
            .unwrap_or_default();
        let start = location.get("range").and_then(|r| r.get("start"));
        out.push(LspSymbol {
            name: name.to_string(),
            kind,
            path,
            line: start.map_or(1, |s| one_indexed(s, "line")),
            character: start.map_or(1, |s| one_indexed(s, "character")),
        });
    } else {
        let start = item
            .get("selectionRange")
            .or_else(|| item.get("range"))
            .and_then(|r| r.get("start"));
        out.push(LspSymbol {
            name: name.to_string(),
            kind,
            // DocumentSymbol entries are scoped to the queried document; the
            // caller already knows the path, so it is left empty here.
            path: String::new(),
            line: start.map_or(1, |s| one_indexed(s, "line")),
            character: start.map_or(1, |s| one_indexed(s, "character")),
        });
    }

    if let Some(children) = item.get("children").and_then(JsonValue::as_array) {
        for child in children {
            collect_symbol(child, out);
        }
    }
}

/// Normalize a `completion` result — either a bare `CompletionItem[]` or a
/// `CompletionList` (`{items: [...]}`) — into [`LspCompletionItem`]s.
fn normalize_completion(raw: &JsonValue) -> Option<Vec<LspCompletionItem>> {
    let items = raw
        .as_array()
        .or_else(|| raw.get("items").and_then(JsonValue::as_array))?;
    if items.is_empty() {
        return None;
    }
    let mut completions = Vec::new();
    for item in items {
        let Some(label) = item.get("label").and_then(JsonValue::as_str) else {
            continue;
        };
        completions.push(LspCompletionItem {
            label: label.to_string(),
            kind: completion_kind_label(item.get("kind").and_then(JsonValue::as_u64)),
            detail: item
                .get("detail")
                .and_then(JsonValue::as_str)
                .map(str::to_string),
            insert_text: item
                .get("insertText")
                .and_then(JsonValue::as_str)
                .map(str::to_string),
        });
    }
    if completions.is_empty() {
        None
    } else {
        Some(completions)
    }
}

/// Read the source line at a 1-indexed line number for a workspace-relative
/// path, trimmed and length-capped, to give the model an at-a-glance preview
/// of each location without a follow-up read. Best-effort: missing files or
/// out-of-range lines yield `None`.
fn preview_line(path: &str, line: u32) -> Option<String> {
    const PREVIEW_MAX: usize = 120;
    if path.is_empty() || line == 0 {
        return None;
    }
    let resolved =
        std::env::current_dir().map_or_else(|_| PathBuf::from(path), |cwd| cwd.join(path));
    let contents = std::fs::read_to_string(resolved).ok()?;
    let index = usize::try_from(line - 1).ok()?;
    let raw = contents.lines().nth(index)?.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.chars().count() > PREVIEW_MAX {
        let truncated: String = raw.chars().take(PREVIEW_MAX).collect();
        Some(format!("{truncated}…"))
    } else {
        Some(raw.to_string())
    }
}

/// Map an LSP `SymbolKind` integer to a lowercase label. Unknown kinds fall
/// back to `symbol`.
fn symbol_kind_label(kind: Option<u64>) -> String {
    match kind {
        Some(1) => "file",
        Some(2) => "module",
        Some(3) => "namespace",
        Some(4) => "package",
        Some(5) => "class",
        Some(6) => "method",
        Some(7) => "property",
        Some(8) => "field",
        Some(9) => "constructor",
        Some(10) => "enum",
        Some(11) => "interface",
        Some(12) => "function",
        Some(13) => "variable",
        Some(14) => "constant",
        Some(15) => "string",
        Some(16) => "number",
        Some(17) => "boolean",
        Some(18) => "array",
        Some(19) => "object",
        Some(20) => "key",
        Some(21) => "null",
        Some(22) => "enum_member",
        Some(23) => "struct",
        Some(24) => "event",
        Some(25) => "operator",
        Some(26) => "type_parameter",
        _ => "symbol",
    }
    .to_string()
}

/// Map an LSP `CompletionItemKind` integer to a lowercase label. Unknown
/// kinds return `None` so the field is omitted rather than mislabeled.
fn completion_kind_label(kind: Option<u64>) -> Option<String> {
    let label = match kind? {
        1 => "text",
        2 => "method",
        3 => "function",
        4 => "constructor",
        5 => "field",
        6 => "variable",
        7 => "class",
        8 => "interface",
        9 => "module",
        10 => "property",
        11 => "unit",
        12 => "value",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        16 => "color",
        17 => "file",
        18 => "reference",
        19 => "folder",
        20 => "enum_member",
        21 => "constant",
        22 => "struct",
        23 => "event",
        24 => "operator",
        25 => "type_parameter",
        _ => return None,
    };
    Some(label.to_string())
}

/// Read one `Content-Length`-framed LSP message body from `reader`.
///
/// LSP (unlike MCP stdio, which is newline-delimited JSON) really is
/// header-framed, so this stays independent of `McpStdioProcess`.
async fn read_lsp_frame(reader: &mut BufReader<ChildStdout>) -> io::Result<Vec<u8>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line).await?;
        if bytes_read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "LSP stdio stream closed while reading headers",
            ));
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("Content-Length") {
                let parsed = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                content_length = Some(parsed);
            }
        }
    }

    let content_length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    let mut payload = vec![0_u8; content_length];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

/// Frame a JSON-RPC payload with the canonical `Content-Length` envelope.
fn encode_lsp_frame(payload: &[u8]) -> Vec<u8> {
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    let mut framed = header.into_bytes();
    framed.extend_from_slice(payload);
    framed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnostic {
    pub path: String,
    pub line: u32,
    pub character: u32,
    pub severity: String,
    pub message: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspLocation {
    pub path: String,
    pub line: u32,
    pub character: u32,
    pub end_line: Option<u32>,
    pub end_character: Option<u32>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspHoverResult {
    pub content: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspCompletionItem {
    pub label: String,
    pub kind: Option<String>,
    pub detail: Option<String>,
    pub insert_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSymbol {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LspServerStatus {
    Connected,
    Disconnected,
    Starting,
    Error,
}

impl std::fmt::Display for LspServerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected => write!(f, "connected"),
            Self::Disconnected => write!(f, "disconnected"),
            Self::Starting => write!(f, "starting"),
            Self::Error => write!(f, "error"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerState {
    pub language: String,
    pub status: LspServerStatus,
    pub root_path: Option<String>,
    pub capabilities: Vec<String>,
    pub diagnostics: Vec<LspDiagnostic>,
    #[serde(skip)]
    pub transport: Option<Arc<dyn LspTransport>>,
}

#[derive(Debug, Clone, Default)]
pub struct LspRegistry {
    inner: Arc<Mutex<RegistryInner>>,
    publish_generation: Arc<AtomicU64>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    servers: HashMap<String, LspServerState>,
    diagnostics_index: HashMap<String, HashMap<String, Vec<LspDiagnostic>>>,
}

fn diagnostics_by_path(diags: &[LspDiagnostic]) -> HashMap<String, Vec<LspDiagnostic>> {
    let mut by_path: HashMap<String, Vec<LspDiagnostic>> = HashMap::new();
    for diagnostic in diags {
        by_path
            .entry(diagnostic.path.clone())
            .or_default()
            .push(diagnostic.clone());
    }
    by_path
}

fn trim_diagnostics(diags: &mut Vec<LspDiagnostic>) {
    let excess = diags.len().saturating_sub(MAX_DIAGNOSTICS_PER_SERVER);
    if excess > 0 {
        diags.drain(0..excess);
    }
}

fn rebuild_diagnostics_index(inner: &mut RegistryInner, language: &str) {
    if let Some(server) = inner.servers.get(language) {
        inner.diagnostics_index.insert(
            language.to_string(),
            diagnostics_by_path(&server.diagnostics),
        );
    } else {
        inner.diagnostics_index.remove(language);
    }
}

impl LspRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        language: &str,
        status: LspServerStatus,
        root_path: Option<&str>,
        capabilities: Vec<String>,
    ) {
        self.register_with_transport(language, status, root_path, capabilities, None);
    }

    pub fn register_with_transport(
        &self,
        language: &str,
        status: LspServerStatus,
        root_path: Option<&str>,
        capabilities: Vec<String>,
        transport: Option<Arc<dyn LspTransport>>,
    ) {
        // Wire the transport's diagnostics callback into this registry via a
        // weak handle so the reader task does not keep the registry alive
        // (avoids a transport <-> registry reference cycle).
        if let Some(transport) = transport.as_ref() {
            let weak = Arc::downgrade(&self.inner);
            let publish_generation = Arc::clone(&self.publish_generation);
            let lang = language.to_owned();
            transport.attach_diagnostics_sink(Box::new(move |diags| {
                publish_generation.fetch_add(1, Ordering::SeqCst);
                if let Some(inner) = weak.upgrade() {
                    if let Ok(mut guard) = inner.lock() {
                        if let Some(server) = guard.servers.get_mut(&lang) {
                            // publishDiagnostics carries the full diagnostic
                            // set for a uri (replace semantics): drop prior
                            // entries for the same path, then insert.
                            let incoming_paths: HashSet<_> =
                                diags.iter().map(|d| d.path.clone()).collect();
                            server
                                .diagnostics
                                .retain(|d| !incoming_paths.contains(&d.path));
                            server.diagnostics.extend(diags);
                            trim_diagnostics(&mut server.diagnostics);
                            let index = diagnostics_by_path(&server.diagnostics);
                            guard.diagnostics_index.insert(lang.clone(), index);
                        }
                    }
                }
            }));
        }

        let mut inner = self.inner.lock().expect("lsp registry lock poisoned");
        inner.servers.insert(
            language.to_owned(),
            LspServerState {
                language: language.to_owned(),
                status,
                root_path: root_path.map(str::to_owned),
                capabilities,
                diagnostics: Vec::new(),
                transport,
            },
        );
        inner
            .diagnostics_index
            .insert(language.to_owned(), HashMap::new());
    }

    #[must_use]
    pub fn get(&self, language: &str) -> Option<LspServerState> {
        let inner = self.inner.lock().expect("lsp registry lock poisoned");
        inner.servers.get(language).cloned()
    }

    /// Find the appropriate server for a file path based on extension.
    #[must_use]
    pub fn find_server_for_path(&self, path: &str) -> Option<LspServerState> {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let language = match ext {
            "rs" => "rust",
            "ts" | "tsx" => "typescript",
            "js" | "jsx" => "javascript",
            "py" => "python",
            "go" => "go",
            "java" => "java",
            "c" | "h" => "c",
            "cpp" | "hpp" | "cc" => "cpp",
            "rb" => "ruby",
            "lua" => "lua",
            _ => return None,
        };

        self.get(language)
    }

    /// List all registered servers.
    #[must_use]
    pub fn list_servers(&self) -> Vec<LspServerState> {
        let inner = self.inner.lock().expect("lsp registry lock poisoned");
        inner.servers.values().cloned().collect()
    }

    /// Add diagnostics to a server.
    pub fn add_diagnostics(
        &self,
        language: &str,
        diagnostics: Vec<LspDiagnostic>,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().expect("lsp registry lock poisoned");
        {
            let server = inner
                .servers
                .get_mut(language)
                .ok_or_else(|| format!("LSP server not found for language: {language}"))?;
            server.diagnostics.extend(diagnostics);
            trim_diagnostics(&mut server.diagnostics);
        }
        rebuild_diagnostics_index(&mut inner, language);
        Ok(())
    }

    /// Get diagnostics for a specific file path.
    #[must_use]
    pub fn get_diagnostics(&self, path: &str) -> Vec<LspDiagnostic> {
        let inner = self.inner.lock().expect("lsp registry lock poisoned");
        inner
            .diagnostics_index
            .values()
            .filter_map(|diagnostics_by_path| diagnostics_by_path.get(path))
            .flat_map(|diagnostics| diagnostics.iter())
            .cloned()
            .collect()
    }

    /// Tell the matching server about changed document text, then wait
    /// briefly for a publishDiagnostics notification before reading cache.
    #[must_use]
    pub fn sync_and_collect_diagnostics(
        &self,
        path: &str,
        text: &str,
        timeout: Duration,
    ) -> Vec<LspDiagnostic> {
        let Some(server) = self.find_server_for_path(path) else {
            return self.get_diagnostics(path);
        };
        let Some(transport) = server.transport else {
            return self.get_diagnostics(path);
        };

        let generation = self.publish_generation.load(Ordering::SeqCst);
        transport.notify_document_changed(path, text);

        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if self.publish_generation.load(Ordering::SeqCst) > generation {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        self.get_diagnostics(path)
    }

    /// Clear diagnostics for a language server.
    pub fn clear_diagnostics(&self, language: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().expect("lsp registry lock poisoned");
        {
            let server = inner
                .servers
                .get_mut(language)
                .ok_or_else(|| format!("LSP server not found for language: {language}"))?;
            server.diagnostics.clear();
        }
        inner.diagnostics_index.remove(language);
        Ok(())
    }

    /// Disconnect a server.
    #[must_use]
    pub fn disconnect(&self, language: &str) -> Option<LspServerState> {
        let mut inner = self.inner.lock().expect("lsp registry lock poisoned");
        inner.diagnostics_index.remove(language);
        inner.servers.remove(language)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().expect("lsp registry lock poisoned");
        inner.servers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Dispatch an LSP action and return a structured result.
    pub async fn dispatch(
        &self,
        action: &str,
        path: Option<&str>,
        line: Option<u32>,
        character: Option<u32>,
        _query: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let lsp_action = LspAction::parse_action(action)
            .ok_or_else(|| format!("unknown LSP action: {action}"))?;

        // For actions requiring a path, validate workspace boundary
        if let Some(path_str) = path {
            let workspace_root = std::env::current_dir().map_err(|e| e.to_string())?;
            let resolved_path = workspace_root.join(path_str);
            let canonical_path = resolved_path.canonicalize().unwrap_or(resolved_path);
            if let Err(e) =
                crate::file_ops::validate_workspace_boundary(&canonical_path, &workspace_root)
            {
                return Err(format!("Security error: {e}"));
            }
        }

        // For diagnostics, we can check existing cached diagnostics
        if lsp_action == LspAction::Diagnostics {
            if let Some(path) = path {
                let diags = self.get_diagnostics(path);
                return Ok(serde_json::json!({
                    "action": "diagnostics",
                    "path": path,
                    "diagnostics": diags,
                    "count": diags.len()
                }));
            }
            // All diagnostics across all servers
            let inner = self.inner.lock().expect("lsp registry lock poisoned");
            let all_diags: Vec<_> = inner
                .servers
                .values()
                .flat_map(|s| &s.diagnostics)
                .collect();
            return Ok(serde_json::json!({
                "action": "diagnostics",
                "diagnostics": all_diags,
                "count": all_diags.len()
            }));
        }

        // For other actions, we need a connected server for the given file
        let path = path.ok_or("path is required for this LSP action")?;
        let server = self
            .find_server_for_path(path)
            .ok_or_else(|| format!("no LSP server available for path: {path}"))?;

        if server.status != LspServerStatus::Connected {
            return Err(format!(
                "LSP server for '{}' is not connected (status: {})",
                server.language, server.status
            ));
        }

        if !server.capabilities.is_empty()
            && !server
                .capabilities
                .iter()
                .any(|capability| LspAction::parse_action(capability) == Some(lsp_action))
        {
            return Err(format!(
                "LSP server for '{}' does not advertise support for {}",
                server.language,
                lsp_action.capability_name()
            ));
        }

        if let Some(transport) = server.transport {
            let raw = timeout(
                Duration::from_millis(LSP_DISPATCH_TIMEOUT_MS),
                transport.dispatch(lsp_action, path, line.unwrap_or(0), character.unwrap_or(0)),
            )
            .await
            .map_err(|_| {
                format!(
                    "LSP dispatch timed out after {} ms for '{}' while handling {}",
                    LSP_DISPATCH_TIMEOUT_MS,
                    server.language,
                    lsp_action.capability_name()
                )
            })??;
            // Reshape raw JSON-RPC (file:// URIs, 0-indexed) into the
            // model-facing structs; see `normalize_dispatch_result`.
            return Ok(normalize_dispatch_result(lsp_action, raw));
        }

        Err(format!(
            "LSP server for '{}' is connected but has no active transport for {}",
            server.language, action
        ))
    }
}

#[cfg(test)]
mod tests;
