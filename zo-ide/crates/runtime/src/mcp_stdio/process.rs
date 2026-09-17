//! Raw child-process MCP transport.
//!
//! [`McpStdioProcess`] owns the spawned child handle plus framed stdin/
//! stdout pipes and exposes the JSON-RPC primitives the manager layer
//! composes on top: `send_request`, `read_response`, plus convenience
//! wrappers (`initialize`, `list_tools`, `call_tool`, `list_resources`,
//! `read_resource`).
//!
//! Framing is newline-delimited JSON (ndjson), per the MCP stdio transport
//! spec: "Messages are delimited by newlines, and MUST NOT contain embedded
//! newlines." This is NOT the LSP `Content-Length: N\r\n\r\n…` envelope —
//! every spec-conforming stdio server (the official SDKs, `mcp-remote`,
//! `chrome-devtools-mcp`, Playwright MCP, …) reads lines from stdin and
//! blocks forever on an LSP-framed request, which pinned every stdio server
//! to "discovering" while remote (HTTP/SSE) servers worked. The LSP client
//! keeps its own Content-Length framing in `lsp_client` — that protocol
//! really is header-framed.

use std::collections::BTreeMap;
use std::io;
use std::process::Stdio;

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::mcp_client::{McpClientBootstrap, McpClientTransport, McpStdioTransport};
use crate::mcp_limits::guard_mcp_read_growth;

use super::types::{
    InboundEvent, JsonRpcId, JsonRpcRequest, JsonRpcResponse, McpInitializeParams,
    McpInitializeResult, McpListResourcesParams, McpListResourcesResult, McpListToolsParams,
    McpListToolsResult, McpReadResourceParams, McpReadResourceResult, McpToolCallParams,
    McpToolCallResult,
};

#[derive(Debug)]
pub struct McpStdioProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Server→client notifications captured while reading responses (the read
    /// loop in [`Self::request`] is the only place that drains stdout, so
    /// notifications can only surface there). Drained by [`Self::poll_inbound`].
    inbound_events: Vec<InboundEvent>,
}

/// Classify a JSON-RPC notification frame (one with no `id`) into the inbound
/// event the consumer chain acts on, or `None` for notifications we ignore.
///
/// Pure and infallible: a malformed or unknown notification yields `None` and
/// must never be allowed to kill the response stream.
pub(crate) fn inbound_event_for_notification(value: &serde_json::Value) -> Option<InboundEvent> {
    match value.get("method").and_then(serde_json::Value::as_str) {
        Some("notifications/tools/list_changed") => Some(InboundEvent::ToolsListChanged),
        _ => None,
    }
}

impl McpStdioProcess {
    pub fn spawn(transport: &McpStdioTransport) -> io::Result<Self> {
        let mut command = Command::new(&transport.command);
        command
            .args(&transport.args)
            // Discard the child's stderr instead of inheriting the terminal's.
            // MCP/LSP servers (rust-analyzer especially) are extremely chatty on
            // stderr — progress, indexing, `notify` warnings — and inheriting it
            // corrupts the TUI alt-screen (and pollutes headless `-p` output).
            // The protocol rides stdout, so the server is unaffected; piping
            // instead would risk a full pipe buffer stalling the server unless
            // drained, so `null` is the safe minimal redirect.
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        apply_env(&mut command, &transport.env);

        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("stdio MCP process missing stdin pipe"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("stdio MCP process missing stdout pipe"))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            inbound_events: Vec::new(),
        })
    }

    /// Record an inbound event, deduplicating so the buffer can never grow
    /// without bound. Every variant is idempotent for the consumer (a refresh
    /// re-reads the server's *current* state), so collapsing duplicates is both
    /// the bound and the correct semantics — one pending `ToolsListChanged` is
    /// indistinguishable from ten.
    fn push_inbound(&mut self, event: InboundEvent) {
        if !self.inbound_events.contains(&event) {
            self.inbound_events.push(event);
        }
    }

    /// Drain the buffered inbound events. The caller (manager poll) decides what
    /// to do with each; an empty `Vec` means nothing changed.
    pub fn poll_inbound(&mut self) -> Vec<InboundEvent> {
        std::mem::take(&mut self.inbound_events)
    }

    /// Decompose the process into its owned child handle and framed pipes.
    ///
    /// Used by the LSP transport to move stdout into a background reader
    /// task while retaining the child + stdin for request/notification
    /// sending. The MCP manager never calls this; the existing methods are
    /// untouched.
    #[must_use]
    pub fn into_parts(self) -> (Child, ChildStdin, BufReader<ChildStdout>) {
        (self.child, self.stdin, self.stdout)
    }

    pub async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stdin.write_all(bytes).await
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.stdin.flush().await
    }

    pub async fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.write_all(line.as_bytes()).await?;
        self.write_all(b"\n").await?;
        self.flush().await
    }

    pub async fn read_line(&mut self) -> io::Result<String> {
        let line = self.read_bounded_line().await?;
        if line.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "MCP stdio stream closed while reading line",
            ));
        }
        Ok(line)
    }

    /// Read one newline-terminated line, rejecting a server that streams past
    /// the shared MCP cap without ever emitting `\n`. Unlike `read_until`/
    /// `read_line` (which buffer the whole unterminated run before returning, so
    /// the cap could only fire after the memory was spent), this consumes the
    /// `BufReader` one chunk at a time via `fill_buf`/`consume`, guarding growth
    /// *before* each append so an oversized frame is rejected without
    /// accumulating past the cap. Returns an empty `String` on EOF.
    async fn read_bounded_line(&mut self) -> io::Result<String> {
        let mut bytes = Vec::new();
        loop {
            let available = self.stdout.fill_buf().await?;
            if available.is_empty() {
                break; // EOF
            }
            // Copy up to and including the next newline; stop the append there so
            // a chunk that spans a frame boundary does not pull the following
            // frame's bytes into this line.
            let (chunk_len, found_newline) = match available.iter().position(|&b| b == b'\n') {
                Some(index) => (index + 1, true),
                None => (available.len(), false),
            };
            guard_mcp_read_growth(bytes.len(), chunk_len, "stdio line")?;
            bytes.extend_from_slice(&available[..chunk_len]);
            self.stdout.consume(chunk_len);
            if found_newline {
                break;
            }
        }
        String::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub async fn read_available(&mut self) -> io::Result<Vec<u8>> {
        let mut buffer = vec![0_u8; 4096];
        let read = self.stdout.read(&mut buffer).await?;
        buffer.truncate(read);
        Ok(buffer)
    }

    pub async fn write_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        let encoded = encode_frame(payload);
        self.write_all(&encoded).await?;
        self.flush().await
    }

    /// Read one newline-delimited JSON-RPC message (MCP stdio framing).
    /// Blank lines are skipped defensively; a trailing `\r` (CRLF-emitting
    /// server) is trimmed. EOF before a message surfaces as `UnexpectedEof`
    /// so the manager can classify the dead process.
    pub async fn read_frame(&mut self) -> io::Result<Vec<u8>> {
        loop {
            let line = self.read_bounded_line().await?;
            if line.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "MCP stdio stream closed while reading message",
                ));
            }
            let message = line.trim();
            if message.is_empty() {
                continue;
            }
            return Ok(message.as_bytes().to_vec());
        }
    }

    pub async fn write_jsonrpc_message<T: Serialize>(&mut self, message: &T) -> io::Result<()> {
        let body = serde_json::to_vec(message)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.write_frame(&body).await
    }

    pub async fn read_jsonrpc_message<T: DeserializeOwned>(&mut self) -> io::Result<T> {
        let payload = self.read_frame().await?;
        serde_json::from_slice(&payload)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub async fn send_request<T: Serialize>(
        &mut self,
        request: &JsonRpcRequest<T>,
    ) -> io::Result<()> {
        self.write_jsonrpc_message(request).await
    }

    pub async fn read_response<T: DeserializeOwned>(&mut self) -> io::Result<JsonRpcResponse<T>> {
        self.read_jsonrpc_message().await
    }

    pub async fn request<TParams: Serialize, TResult: DeserializeOwned>(
        &mut self,
        id: JsonRpcId,
        method: impl Into<String>,
        params: Option<TParams>,
    ) -> io::Result<JsonRpcResponse<TResult>> {
        let method = method.into();
        let request = JsonRpcRequest::new(id.clone(), method.clone(), params);
        self.send_request(&request).await?;

        // 우리 요청 id 와 매칭되는 응답이 올 때까지 프레임을 소비한다. MCP
        // 스펙은 서버가 응답 스트림에 notification(notifications/progress·
        // message 등, id 필드 없음)이나 다른 요청의 응답을 끼워 보내는 것을
        // 허용한다. 구버전은 첫 프레임 1개만 읽고 id 불일치 시 즉시
        // InvalidData 로 실패해, 그런 스펙 준수 서버를 transport 오류 →
        // 서버 재시작 루프로 몰아넣었다. notification 은 건너뛰고 매칭 응답만
        // 반환한다. (무한 대기는 호출부의 with_timeout 가드가 끊는다 —
        // `mcp_stdio::mod.rs` 의 tool_call timeout 등.)
        let expected_id = serde_json::to_value(&id)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

        loop {
            let payload = self.read_frame().await?;
            let value: serde_json::Value = serde_json::from_slice(&payload)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

            match value.get("id") {
                // id 필드 부재 = JSON-RPC notification. 응답은 아니지만, 소비
                // 대상(tools/list_changed)이면 버퍼에 담고 나머지는 무시한다.
                // 파싱은 infallible — 깨진 notification 이 스트림을 죽이면 안 된다.
                None => {
                    if let Some(event) = inbound_event_for_notification(&value) {
                        self.push_inbound(event);
                    }
                    continue;
                }
                // 다른 요청의 응답 — 단일 in-flight 모델에선 예상 밖이나, 우리
                // 응답을 계속 기다리기 위해 버린다.
                Some(frame_id) if *frame_id != expected_id => continue,
                Some(_) => {}
            }

            let response: JsonRpcResponse<TResult> = serde_json::from_slice(&payload)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

            if response.jsonrpc != "2.0" {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "MCP response for {method} used unsupported jsonrpc version `{}`",
                        response.jsonrpc
                    ),
                ));
            }

            return Ok(response);
        }
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
        self.write_jsonrpc_message(&notification).await
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

    pub async fn terminate(&mut self) -> io::Result<()> {
        self.child.kill().await
    }

    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    pub fn has_exited(&mut self) -> io::Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }

    pub(super) async fn shutdown(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_none() {
            match self.child.kill().await {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
                Err(error) => return Err(error),
            }
        }
        let _ = self.child.wait().await?;
        Ok(())
    }
}

/// Spawn a stdio MCP child process from the bootstrap config. The
/// non-stdio variants are rejected — those are routed through the
/// manager's `open_mcp_process` dispatcher in `mod.rs`.
pub fn spawn_mcp_stdio_process(bootstrap: &McpClientBootstrap) -> io::Result<McpStdioProcess> {
    if bootstrap.is_project_scoped {
        eprintln!(
            "Warning: MCP server '{}' is defined in project-scoped config and will execute command: {}",
            bootstrap.server_name,
            match &bootstrap.transport {
                McpClientTransport::Stdio(t) => t.command.as_str(),
                _ => "<non-stdio>",
            }
        );
    }
    match &bootstrap.transport {
        McpClientTransport::Stdio(transport) => {
            let transport = stdio_transport_with_audit_env(transport, bootstrap);
            McpStdioProcess::spawn(&transport)
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "MCP bootstrap transport for {} is not stdio: {other:?}",
                bootstrap.server_name
            ),
        )),
    }
}

fn stdio_transport_with_audit_env(
    transport: &McpStdioTransport,
    bootstrap: &McpClientBootstrap,
) -> McpStdioTransport {
    let mut transport = transport.clone();
    transport.env.insert(
        "ZO_MCP_SERVER_NAME".to_string(),
        bootstrap.server_name.clone(),
    );
    transport.env.insert(
        "ZO_MCP_SERVER_NORMALIZED_NAME".to_string(),
        bootstrap.normalized_name.clone(),
    );
    transport.env.insert(
        "ZO_MCP_TOOL_PREFIX".to_string(),
        bootstrap.tool_prefix.clone(),
    );
    transport
        .env
        .insert("ZO_MCP_TRANSPORT".to_string(), "stdio".to_string());
    transport.env.insert(
        "ZO_MCP_PROJECT_SCOPED".to_string(),
        bootstrap.is_project_scoped.to_string(),
    );
    if let Ok(cwd) = std::env::current_dir() {
        transport
            .env
            .insert("ZO_MCP_CWD".to_string(), cwd.display().to_string());
    }
    transport
}

fn apply_env(command: &mut Command, env: &BTreeMap<String, String>) {
    for (key, value) in env {
        command.env(key, value);
    }
    // PATH augmentation (the "npx not found" fix). A stdio MCP server is usually
    // launched as a bare command name (`npx`, `node`, `uvx`, `python`); the child
    // inherits this process's PATH, so when zo itself is started from a GUI
    // launcher (Finder/app icon / `launchd`) rather than a login shell, PATH is
    // the impoverished system default (`/usr/bin:/bin:…`) that omits Homebrew
    // (`/opt/homebrew/bin`) and other user tool dirs — and every `npx`-based
    // server fails to spawn with ENOENT and is stuck "discovering" forever, while
    // a remote (URL) server like context7 is unaffected.
    //
    // Fix: append the standard tool directories that actually exist to the child's
    // PATH so a bare `npx`/`node` resolves regardless of how zo was launched.
    // A server that pins its own `env.PATH` is left exactly as it set it (its
    // explicit choice wins); we only augment the inherited PATH.
    if !env.contains_key("PATH") {
        let inherited = std::env::var("PATH").unwrap_or_default();
        if let Some(merged) = merge_path_dirs(&inherited, &standard_tool_path_dirs()) {
            command.env("PATH", merged);
        }
    }
}

/// Standard tool directories to graft onto an inherited PATH, in priority order,
/// filtered to those that actually exist on this machine. Covers Homebrew (Apple
/// Silicon + Intel), the common Node/`npm -g`/`uvx`/Cargo bin locations, and the
/// system defaults — the places a `npx`/`node`/`python`/`uvx` MCP launcher lives.
fn standard_tool_path_dirs() -> Vec<String> {
    let mut dirs = vec![
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/local/sbin".to_string(),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::Path::new(&home);
        for sub in [".local/bin", ".cargo/bin", ".npm-global/bin", ".bun/bin"] {
            dirs.push(home.join(sub).display().to_string());
        }
    }
    dirs.extend(["/usr/bin".to_string(), "/bin".to_string()]);
    dirs.into_iter()
        .filter(|dir| std::path::Path::new(dir).is_dir())
        .collect()
}

/// Append `extra` directories to a PATH string, preserving the inherited entries'
/// priority (they come first and win) and dropping any `extra` already present so
/// no directory is duplicated. Returns `None` when nothing new would be added, so
/// the caller can leave the child's PATH untouched in the common case where the
/// launch environment already had everything.
fn merge_path_dirs(inherited: &str, extra: &[String]) -> Option<String> {
    let separator = ':';
    let existing: Vec<&str> = inherited.split(separator).filter(|s| !s.is_empty()).collect();
    let additions: Vec<&String> = extra
        .iter()
        .filter(|dir| !dir.is_empty() && !existing.contains(&dir.as_str()))
        .collect();
    if additions.is_empty() {
        return None;
    }
    let mut merged = String::from(inherited);
    for dir in additions {
        if !merged.is_empty() {
            merged.push(separator);
        }
        merged.push_str(dir);
    }
    Some(merged)
}

/// Frame one JSON-RPC payload for the MCP stdio transport: the compact JSON
/// bytes plus a terminating `\n`. `serde_json::to_vec` never emits raw
/// newlines (they are escaped inside strings), which is exactly the spec's
/// "MUST NOT contain embedded newlines" requirement.
fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(payload.len() + 1);
    framed.extend_from_slice(payload);
    framed.push(b'\n');
    framed
}

#[cfg(test)]
mod tests {
    use super::{inbound_event_for_notification, merge_path_dirs, InboundEvent, McpStdioProcess};
    use crate::mcp_client::McpStdioTransport;
    use crate::mcp_limits::MAX_MCP_MESSAGE_BYTES;
    use serde_json::json;

    /// Spawn a shell child whose stdout is driven by `script`, wrapped as an
    /// [`McpStdioProcess`] so the real bounded-line read path is exercised end to
    /// end. Unix-only: the tests assert POSIX `sh` behavior.
    #[cfg(unix)]
    fn spawn_sh(script: &str) -> McpStdioProcess {
        let transport = McpStdioTransport {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            env: std::collections::BTreeMap::new(),
            tool_call_timeout_ms: None,
        };
        McpStdioProcess::spawn(&transport).expect("spawn sh child")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_bounded_line_reads_a_normal_newline_terminated_line() {
        let mut process = spawn_sh("printf 'hello world\\n'");
        let line = process.read_bounded_line().await.expect("a line");
        assert_eq!(line, "hello world\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_bounded_line_joins_input_split_across_chunks() {
        // The line arrives in two writes with a delay between, so the first
        // `fill_buf` returns only the head (no newline) and the loop must keep
        // reading. Both halves must land in the single returned line.
        // Use `printf '%s'` for the second write so its `-second-half` argument
        // is not parsed as a `printf` option flag (a bare `printf '-second…'`
        // errors and emits nothing, which silently truncates the input).
        let mut process =
            spawn_sh("printf 'first-half'; sleep 0.1; printf '%s\\n' '-second-half'");
        let line = process.read_bounded_line().await.expect("a joined line");
        assert_eq!(line, "first-half-second-half\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_bounded_line_rejects_a_stream_that_never_emits_a_newline() {
        // `yes` streams `x\n` forever, but with newlines stripped it is an
        // unterminated run of bytes far past the cap — the exact case
        // `read_until` would buffer without bound. The chunked reader must
        // reject it (InvalidData) without accumulating past the cap, and must do
        // so promptly rather than hanging.
        let mut process = spawn_sh("yes x | tr -d '\\n'");
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            process.read_bounded_line(),
        )
        .await
        .expect("the bounded read must return, not hang")
        .expect_err("an unterminated over-cap stream must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("stdio line"),
            "error should name the read site: {error}"
        );
        let _ = process.terminate().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_bounded_line_accepts_a_line_at_exactly_the_cap() {
        // A line of exactly the cap (minus the newline) is within budget and
        // must be accepted — the boundary the cap+1 rejection sits just past.
        let payload = MAX_MCP_MESSAGE_BYTES - 1;
        let mut process = spawn_sh(&format!("head -c {payload} /dev/zero | tr '\\0' 'a'; printf '\\n'"));
        let line = process.read_bounded_line().await.expect("a cap-sized line");
        assert_eq!(line.len(), MAX_MCP_MESSAGE_BYTES);
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn merge_path_appends_missing_dirs_after_inherited_ones() {
        // The fix's core: a Homebrew bin missing from a GUI-launch PATH is grafted
        // on AFTER the inherited entries (which keep priority), so a bare `npx`
        // resolves without the inherited resolution order changing.
        let merged = merge_path_dirs(
            "/usr/bin:/bin",
            &["/opt/homebrew/bin".to_string(), "/usr/local/bin".to_string()],
        )
        .expect("a new dir is added");
        assert_eq!(merged, "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin");
    }

    #[test]
    fn merge_path_drops_duplicates_and_preserves_order() {
        // A dir already on PATH is never re-appended (no duplication / churn);
        // only the genuinely-new dir lands, and the inherited order is untouched.
        let merged = merge_path_dirs(
            "/opt/homebrew/bin:/usr/bin",
            &["/opt/homebrew/bin".to_string(), "/usr/local/bin".to_string()],
        )
        .expect("one new dir is added");
        assert_eq!(merged, "/opt/homebrew/bin:/usr/bin:/usr/local/bin");
    }

    #[test]
    fn merge_path_returns_none_when_nothing_new() {
        // The common case (a login-shell launch that already had everything):
        // returns None so the child's PATH is left entirely untouched.
        assert_eq!(
            merge_path_dirs(
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin",
                &["/opt/homebrew/bin".to_string(), "/usr/local/bin".to_string()],
            ),
            None,
        );
    }

    #[test]
    fn merge_path_handles_empty_inherited() {
        // An empty inherited PATH (rare, but possible under `env -i`-style launch)
        // still yields the additions without a leading separator.
        let merged = merge_path_dirs("", &["/opt/homebrew/bin".to_string()])
            .expect("a dir is added to an empty PATH");
        assert_eq!(merged, "/opt/homebrew/bin");
    }

    #[test]
    fn classifies_tools_list_changed_notification() {
        let value = json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"});
        assert_eq!(
            inbound_event_for_notification(&value),
            Some(InboundEvent::ToolsListChanged)
        );
    }

    #[test]
    fn ignores_unconsumed_and_malformed_notifications() {
        for value in [
            json!({"jsonrpc": "2.0", "method": "notifications/progress"}),
            json!({"jsonrpc": "2.0", "method": "notifications/resources/list_changed"}),
            json!({"jsonrpc": "2.0"}),
            json!({"jsonrpc": "2.0", "method": 42}),
            json!("not even an object"),
        ] {
            assert_eq!(inbound_event_for_notification(&value), None, "{value}");
        }
    }
}
