//! Tiny test-local Anthropic SSE scripts for responses the shared mock does
//! not need to own (headings, tables, and the exact Bash+Read pair).

use std::io;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
enum Script {
    #[allow(dead_code)] // Used by the separate quality_baseline binary.
    Baseline { task: String, root: String, patch: String },
    Text(String),
    BashRead { final_text: String },
    /// One `Agent` spawn, then a final answer. Drives the delegation cell and
    /// the mid-turn steering path.
    Spawn { final_text: String },
    /// One slow `bash`, then a final answer — a turn that is genuinely BUSY
    /// long enough for a human to type into it.
    SlowTool { final_text: String },
    /// One `bash` call running exactly `command`, then a final answer.
    Bash { command: String, final_text: String },
    /// One `bash` call running exactly `command` with `run_in_background`,
    /// then a final answer — for every later request too, because the task's
    /// completion re-enters as a follow-up turn of its own (t-3177).
    #[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
    BackgroundBash { command: String, final_text: String },
    /// `count` requests answered with one `bash` call each running `command`
    /// with `{n}` replaced by the request index (the runtime skips an exact
    /// repeat of a call, so each one has to differ), then a final answer — a
    /// turn long enough for the context ladder to fire. Every request after
    /// the first is held for `hold` before it is answered, so whatever the
    /// runtime says between requests stands on the status row for at least
    /// that long (t-3177).
    #[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
    ManyBash {
        count: usize,
        command: String,
        hold: Duration,
        final_text: String,
    },
    /// A plain first answer, THEN one `bash` call running exactly `command`
    /// on the next request, then a final answer — a turn that finishes clean
    /// and a second turn that parks on an approval, so a test can subscribe
    /// to a pane's channel between the two.
    #[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
    TextThenBash {
        first_text: String,
        command: String,
        final_text: String,
    },
    /// One call to the named MCP tool with `{"q": "tokio"}`, then a final
    /// answer — a pane child reaching its parent's MCP runtime (t-2513).
    #[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
    McpTool { name: String, final_text: String },
    /// A `bash` that sleeps far longer than any test wants to wait, then a
    /// final answer. The point is not the tool's output — it is that the turn
    /// stays parked between "the `tool_use` was persisted" and "a result
    /// exists", which is the window a window restart kills zo in.
    ParkedTool { seconds: u32, final_text: String },
    /// One assistant answer streamed in pieces with a gap between them, so a
    /// test can kill zo while the text is still arriving.
    DribbledText { text: String, pieces: usize, gap: Duration },
    /// A committed context-trim notice followed by a slow tool on the next
    /// turn. This reproduces the screen shape from the Zed report: the notice
    /// is directly above a live spinner when the terminal grows.
    ContextTrimThenSlowTool,
    /// A real Workflow fan-out whose two children each run one slow Bash.
    WorkflowTwoHelpers { final_text: String },
    /// The host's own pre-analysis fan-out on a `Large` turn: the decomposition
    /// classifier answers two lanes, each lane answers one finding, and the
    /// main turn — which must carry both findings — gets `final_text`.
    HostPreludeTwoLanes { final_text: String },
    /// The parent reads `fixture.txt`, then delegates the same question
    /// twice with `background: false`: to a `fork` — which inherits the read
    /// and must answer without a tool — and to a plain agent, which has to
    /// read again. A child is told from the parent by the question standing
    /// in a USER text block; the parent only ever carries it inside a
    /// `tool_use` input (t-2875).
    #[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
    ForkInheritsParentReading { final_text: String },
    /// `calls[i]` on request `i`, then `final_text` on every later request:
    /// a turn that calls the scheduling tools one after another, and the
    /// turn the session's scheduler opens later, which gets a plain answer.
    #[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
    ToolCalls { calls: Vec<(String, String)>, final_text: String },
    /// `files` written with `write_file` and, after them, one `bash` running
    /// `command` — all announced in ONE assistant message, the batch shape
    /// whose `bash` announce used to close the running writes as failed —
    /// then `final_text`, held back for `hold` so the finished edit cell
    /// stands live on screen that long (t-3063).
    #[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
    WritesThenBash {
        files: Vec<(String, String)>,
        command: Option<String>,
        hold: Duration,
        final_text: String,
    },
}

impl Script {
    fn response(&self, request_index: usize, request: &str) -> String {
        match self {
            Self::Baseline { task, root, patch } => baseline_response(task, root, patch, request),
            Self::Text(text) => text_sse("msg_text_script", text),
            Self::BashRead { .. } if request_index == 0 => bash_read_sse(),
            Self::BashRead { final_text } => text_sse("msg_bash_read_final", final_text),
            Self::Bash { command, .. } if request_index == 0 => bash_sse(command),
            Self::Bash { final_text, .. } => text_sse("msg_bash_final", final_text),
            Self::BackgroundBash { command, .. } if request_index == 0 => {
                background_bash_sse(command)
            }
            Self::BackgroundBash { final_text, .. } => {
                text_sse("msg_background_bash_final", final_text)
            }
            Self::ManyBash { count, command, .. } if request_index < *count => {
                let command = command.replace("{n}", &request_index.to_string());
                bash_sse_for(&format!("toolu_many_bash_{request_index}"), &command)
            }
            Self::ManyBash { final_text, .. } => text_sse("msg_many_bash_final", final_text),
            Self::TextThenBash { first_text, .. } if request_index == 0 => {
                text_sse("msg_text_then_bash_first", first_text)
            }
            Self::TextThenBash { command, .. } if request_index == 1 => bash_sse(command),
            Self::TextThenBash { final_text, .. } => text_sse("msg_text_then_bash_final", final_text),
            Self::McpTool { name, .. } if request_index == 0 => {
                let input = json!({"q": "tokio"}).to_string();
                tool_message_sse("msg_mcp_tool", |body| {
                    append_tool_use(body, 0, "toolu_mcp_e2e", name, &input);
                })
            }
            Self::McpTool { final_text, .. } => text_sse("msg_mcp_final", final_text),
            Self::Spawn { .. } if request_index == 0 => spawn_sse(),
            Self::Spawn { final_text } => text_sse("msg_spawn_final", final_text),
            Self::SlowTool { .. } if request_index == 0 => slow_tool_sse(),
            Self::SlowTool { final_text } => text_sse("msg_slow_final", final_text),
            Self::ParkedTool { seconds, .. } if request_index == 0 => parked_tool_sse(*seconds),
            Self::ParkedTool { final_text, .. } => text_sse("msg_parked_final", final_text),
            Self::DribbledText { text, pieces, .. } if request_index == 0 => {
                dribbled_text_sse("msg_dribbled", text, *pieces)
            }
            Self::DribbledText { text, .. } => text_sse("msg_dribbled_again", text),
            Self::ContextTrimThenSlowTool if request_index == 0 => text_sse(
                "msg_context_trim",
                "Context trim · cleared 3 old tool result(s) (~12k tokens freed)\n",
            ),
            Self::ContextTrimThenSlowTool if request_index == 1 => slow_tool_sse(),
            Self::ContextTrimThenSlowTool => {
                text_sse("msg_context_trim_slow_final", "### Done\n\n- settled\n")
            }
            Self::WorkflowTwoHelpers { final_text } => {
                workflow_two_helpers_response(request_index, request, final_text)
            }
            Self::HostPreludeTwoLanes { final_text } => host_prelude_response(request, final_text),
            Self::ForkInheritsParentReading { final_text } => {
                fork_inherits_response(request, final_text)
            }
            Self::ToolCalls { calls, .. } if request_index < calls.len() => {
                let (name, input) = &calls[request_index];
                let id = format!("toolu_scheduled_{request_index}");
                tool_message_sse(&format!("msg_tool_calls_{request_index}"), |body| {
                    append_tool_use(body, 0, &id, name, input);
                })
            }
            Self::ToolCalls { final_text, .. } => text_sse("msg_tool_calls_final", final_text),
            Self::WritesThenBash { files, command, .. } if request_index == 0 => {
                tool_message_sse("msg_writes_then_bash", |body| {
                    for (index, (path, content)) in files.iter().enumerate() {
                        let input = json!({"path": path, "content": content}).to_string();
                        append_tool_use(body, index, &format!("toolu_write_{index}"), "write_file", &input);
                    }
                    if let Some(command) = command {
                        let input = json!({"command": command, "timeout": 10_000}).to_string();
                        append_tool_use(body, files.len(), "toolu_bash_after_writes", "bash", &input);
                    }
                })
            }
            Self::WritesThenBash { final_text, .. } => text_sse("msg_writes_then_bash_final", final_text),
        }
    }

    /// How long the server sits on request `request_index` before answering
    /// it — the window a test needs the previous turn's last cell to stay
    /// live on screen.
    fn delay_before(&self, request_index: usize) -> Duration {
        match self {
            Self::WritesThenBash { hold, .. } if request_index == 1 => *hold,
            Self::ManyBash { hold, .. } if request_index >= 1 => *hold,
            _ => Duration::ZERO,
        }
    }

    /// How the body is handed to the client: one write, or `pieces` writes with
    /// `gap` between them.
    ///
    /// Only the first request of a dribbling script is slowed — the resumed
    /// turn wants to *finish*, and making the test wait out the gap twice buys
    /// nothing.
    fn pacing(&self, request_index: usize) -> (usize, Duration) {
        match self {
            Self::DribbledText { pieces, gap, .. } if request_index == 0 => (*pieces, *gap),
            _ => (1, Duration::ZERO),
        }
    }
}

/// A deterministic loopback server with a fixed response script.
pub struct ScriptedAnthropicService {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    #[allow(dead_code)] // Baseline counters, shared service in several test binaries.
    usage: Arc<Mutex<[u64; 3]>>,
    shutdown: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<()>,
}

impl ScriptedAnthropicService {
    /// Fixed tool sequences for the quality baseline; never contact a provider.
    #[allow(dead_code)] // quality_baseline only
    pub async fn baseline(task: &str, root: &str, patch: &str) -> io::Result<Self> {
        Self::spawn(Script::Baseline { task: task.into(), root: root.into(), patch: patch.into() }).await
    }

    /// Respond with one text message containing exactly `text`.
    pub async fn text(text: impl Into<String>) -> io::Result<Self> {
        Self::spawn(Script::Text(text.into())).await
    }

    /// Respond first with Bash and Read tool calls, then with `final_text`.
    pub async fn bash_read(final_text: impl Into<String>) -> io::Result<Self> {
        Self::spawn(Script::BashRead {
            final_text: final_text.into(),
        })
        .await
    }

    /// Respond first with one `bash` call running exactly `command`, then with
    /// `final_text`.
    pub async fn bash(
        command: impl Into<String>,
        final_text: impl Into<String>,
    ) -> io::Result<Self> {
        Self::spawn(Script::Bash {
            command: command.into(),
            final_text: final_text.into(),
        })
        .await
    }

    /// Respond first with one `bash` call running exactly `command` in the
    /// background, then with `final_text` for every request after it.
    #[allow(dead_code)] // e2e_hermetic only
    pub async fn background_bash(
        command: impl Into<String>,
        final_text: impl Into<String>,
    ) -> io::Result<Self> {
        Self::spawn(Script::BackgroundBash {
            command: command.into(),
            final_text: final_text.into(),
        })
        .await
    }

    /// Respond to the first `count` requests with one `bash` call each
    /// running `command` (`{n}` is the request index), holding every request
    /// after the first for `hold`, then with `final_text`.
    #[allow(dead_code)] // e2e_hermetic only
    pub async fn many_bash(
        count: usize,
        command: impl Into<String>,
        hold: Duration,
        final_text: impl Into<String>,
    ) -> io::Result<Self> {
        Self::spawn(Script::ManyBash {
            count,
            command: command.into(),
            hold,
            final_text: final_text.into(),
        })
        .await
    }

    /// Respond with `first_text`, then with one `bash` call running exactly
    /// `command`, then with `final_text`.
    #[allow(dead_code)] // e2e_hermetic only
    pub async fn text_then_bash(
        first_text: impl Into<String>,
        command: impl Into<String>,
        final_text: impl Into<String>,
    ) -> io::Result<Self> {
        Self::spawn(Script::TextThenBash {
            first_text: first_text.into(),
            command: command.into(),
            final_text: final_text.into(),
        })
        .await
    }

    /// Respond first with one call to the MCP tool `name`, then with
    /// `final_text`.
    #[allow(dead_code)] // e2e_hermetic only
    pub async fn mcp_tool(name: impl Into<String>, final_text: impl Into<String>) -> io::Result<Self> {
        Self::spawn(Script::McpTool {
            name: name.into(),
            final_text: final_text.into(),
        })
        .await
    }

    /// Respond first with one `Agent` spawn, then with `final_text`.
    pub async fn spawn_agent(final_text: impl Into<String>) -> io::Result<Self> {
        Self::spawn(Script::Spawn {
            final_text: final_text.into(),
        })
        .await
    }

    /// Respond first with a `bash` call that takes seconds, then `final_text`.
    pub async fn slow_tool(final_text: impl Into<String>) -> io::Result<Self> {
        Self::spawn(Script::SlowTool {
            final_text: final_text.into(),
        })
        .await
    }

    /// Respond first with a `bash` call that parks for `seconds`, then
    /// `final_text` on every later request.
    pub async fn parked_tool(seconds: u32, final_text: impl Into<String>) -> io::Result<Self> {
        Self::spawn(Script::ParkedTool {
            seconds,
            final_text: final_text.into(),
        })
        .await
    }

    /// Stream one answer in `pieces` writes separated by `gap`, so a test can
    /// kill the client while the assistant text is still on the wire.
    pub async fn dribbled_text(
        text: impl Into<String>,
        pieces: usize,
        gap: Duration,
    ) -> io::Result<Self> {
        Self::spawn(Script::DribbledText {
            text: text.into(),
            pieces,
            gap,
        })
        .await
    }

    /// Commit a context-trim line, then keep the next turn live in a tool.
    pub async fn context_trim_then_slow_tool() -> io::Result<Self> {
        Self::spawn(Script::ContextTrimThenSlowTool).await
    }

    /// Run a Workflow whose two child prompts are routed by marker.
    pub async fn workflow_two_helpers(final_text: impl Into<String>) -> io::Result<Self> {
        Self::spawn(Script::WorkflowTwoHelpers {
            final_text: final_text.into(),
        })
        .await
    }

    /// Answer the host's pre-analysis fan-out (decomposition, two lanes) and
    /// then the main turn with `final_text`.
    pub async fn host_prelude_two_lanes(final_text: impl Into<String>) -> io::Result<Self> {
        Self::spawn(Script::HostPreludeTwoLanes {
            final_text: final_text.into(),
        })
        .await
    }

    /// The fork scenario (t-2875): see [`Script::ForkInheritsParentReading`].
    #[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
    pub async fn fork_inherits_parent_reading(final_text: impl Into<String>) -> io::Result<Self> {
        Self::spawn(Script::ForkInheritsParentReading {
            final_text: final_text.into(),
        })
        .await
    }

    /// One tool call per request in order, then `final_text` for every
    /// request after them — the scheduling scenarios (t-2901).
    #[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
    pub async fn tool_calls(
        calls: &[(&str, Value)],
        final_text: impl Into<String>,
    ) -> io::Result<Self> {
        Self::spawn(Script::ToolCalls {
            calls: calls
                .iter()
                .map(|(name, input)| ((*name).to_string(), input.to_string()))
                .collect(),
            final_text: final_text.into(),
        })
        .await
    }

    /// Respond first with one message that writes `files` and then runs
    /// `command` (when given), then — after `hold` — with `final_text`.
    #[allow(dead_code)] // e2e_hermetic only
    pub async fn writes_then_bash(
        files: Vec<(String, String)>,
        command: Option<&str>,
        hold: Duration,
        final_text: impl Into<String>,
    ) -> io::Result<Self> {
        Self::spawn(Script::WritesThenBash {
            files,
            command: command.map(str::to_string),
            hold,
            final_text: final_text.into(),
        })
        .await
    }

    /// The URL consumed by `ANTHROPIC_BASE_URL`.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Cumulative SSE usage emitted by the loopback service.
    #[allow(dead_code)] // quality_baseline only
    pub async fn usage(&self) -> [u64; 3] { *self.usage.lock().await }

    /// Bodies of POST requests in arrival order. A second turn must contain
    /// the tool results before the final scripted answer is accepted.
    pub async fn request_bodies(&self) -> Vec<String> {
        self.requests.lock().await.clone()
    }

    async fn spawn(script: Script) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let usage = Arc::new(Mutex::new([0; 3]));
        let response_usage = Arc::clone(&usage);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let request_state = Arc::clone(&requests);
        let join_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break; };
                        let request_state = Arc::clone(&request_state);
                        let script = script.clone();
                        let response_usage = Arc::clone(&response_usage);
                        tokio::spawn(async move {
                            let _ = handle_connection(socket, request_state, response_usage, &script).await;
                        });
                    }
                }
            }
        });

        Ok(Self {
            base_url: format!("http://{address}"),
            requests,
            usage,
            shutdown: Some(shutdown_tx),
            join_handle,
        })
    }
}

impl Drop for ScriptedAnthropicService {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.join_handle.abort();
    }
}

async fn handle_connection(
    mut socket: TcpStream,
    requests: Arc<Mutex<Vec<String>>>,
    usage: Arc<Mutex<[u64; 3]>>,
    script: &Script,
) -> io::Result<()> {
    let (method, body) = read_http_request(&mut socket).await?;
    if method.eq_ignore_ascii_case("POST") {
        let (request_index, response) = {
            let mut captured = requests.lock().await;
            let request_index = captured.len();
            let response = script.response(request_index, &body);
            captured.push(body);
            (request_index, response)
        };
        let mut emitted = [0_u64; 3];
        for line in response.lines().filter_map(|line| line.strip_prefix("data: ")) {
            if let Ok(event) = serde_json::from_str::<Value>(line) {
                let u = if event["type"] == "message_start" { &event["message"]["usage"] } else { &event["usage"] };
                for (index, key) in ["input_tokens", "output_tokens", "cache_read_input_tokens"].iter().enumerate() {
                    emitted[index] = emitted[index].max(u[key].as_u64().unwrap_or(0));
                }
            }
        }
        { let mut total = usage.lock().await; for (i, count) in emitted.iter().enumerate() { total[i] += count; } }
        let (pieces, gap) = script.pacing(request_index);
        let hold = script.delay_before(request_index);
        if !hold.is_zero() {
            tokio::time::sleep(hold).await;
        }
        let framed = http_response("text/event-stream", &response);
        for (index, piece) in split_evenly(&framed, pieces).into_iter().enumerate() {
            if index > 0 {
                tokio::time::sleep(gap).await;
            }
            socket.write_all(piece.as_bytes()).await?;
            socket.flush().await?;
        }
    } else {
        // Some clients optimistically probe the endpoint with HEAD. It must
        // never consume a scripted POST response.
        socket
            .write_all(http_response("text/plain", "").as_bytes())
            .await?;
    }
    Ok(())
}

async fn read_http_request(socket: &mut TcpStream) -> io::Result<(String, String)> {
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request closed before headers",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
    };

    let header_text = String::from_utf8(buffer[..header_end].to_vec())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?;
    let method = request_line
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing method"))?
        .to_string();
    let mut content_length = 0_usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid content-length: {error}"),
                    )
                })?;
            }
        }
    }

    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < content_length {
        let mut chunk = vec![0_u8; content_length - body.len()];
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request closed before body",
            ));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    let body = String::from_utf8(body)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    Ok((method, body))
}

/// Split a response into `pieces` writes on char boundaries.
///
/// The split is deliberately byte-arithmetic rather than SSE-aware: a client
/// that only parses whole events when they happen to arrive whole is a client
/// that will break on a real network, and this harness exists to catch that.
fn split_evenly(body: &str, pieces: usize) -> Vec<&str> {
    if pieces <= 1 {
        return vec![body];
    }
    let mut out = Vec::with_capacity(pieces);
    let mut rest = body;
    for remaining in (1..=pieces).rev() {
        if rest.is_empty() {
            break;
        }
        let mut take = rest.len() / remaining;
        while take < rest.len() && !rest.is_char_boundary(take) {
            take += 1;
        }
        let (head, tail) = rest.split_at(take.min(rest.len()));
        out.push(head);
        rest = tail;
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn http_response(content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn text_sse(message_id: &str, text: &str) -> String {
    let mut body = String::new();
    append_sse(
        &mut body,
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": "claude-sonnet-4-6",
                "stop_reason": null,
                "stop_sequence": null,
                "usage": usage_json(12, 0)
            }
        }),
    );
    append_sse(
        &mut body,
        "content_block_start",
        &json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
    );
    append_sse(
        &mut body,
        "content_block_delta",
        &json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": text}
        }),
    );
    append_sse(
        &mut body,
        "content_block_stop",
        &json!({"type": "content_block_stop", "index": 0}),
    );
    append_sse(
        &mut body,
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": null},
            "usage": usage_json(12, 8)
        }),
    );
    append_sse(&mut body, "message_stop", &json!({"type": "message_stop"}));
    body
}

/// The same answer as [`text_sse`], but cut into `pieces` `text_delta` events
/// so a reader that dies partway through has seen a genuine prefix — one delta
/// carrying the whole answer would make the death all-or-nothing and the
/// variant under test would not exist.
fn dribbled_text_sse(message_id: &str, text: &str, pieces: usize) -> String {
    let mut body = String::new();
    append_sse(
        &mut body,
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": "claude-sonnet-4-6",
                "stop_reason": null,
                "stop_sequence": null,
                "usage": usage_json(12, 0)
            }
        }),
    );
    append_sse(
        &mut body,
        "content_block_start",
        &json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
    );
    for fragment in split_evenly(text, pieces.max(1)) {
        append_sse(
            &mut body,
            "content_block_delta",
            &json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": fragment}
            }),
        );
    }
    append_sse(
        &mut body,
        "content_block_stop",
        &json!({"type": "content_block_stop", "index": 0}),
    );
    append_sse(
        &mut body,
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": null},
            "usage": usage_json(12, 8)
        }),
    );
    append_sse(&mut body, "message_stop", &json!({"type": "message_stop"}));
    body
}

/// One assistant message that stops on `tool_use`: the `message_start`, the
/// tool calls `tools` appends, the `message_delta` that ends the turn on tools,
/// and `message_stop` — the envelope every tool script shares.
fn tool_message_sse(message_id: &str, tools: impl FnOnce(&mut String)) -> String {
    let mut body = String::new();
    append_sse(
        &mut body,
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": "claude-sonnet-4-6",
                "stop_reason": null,
                "stop_sequence": null,
                "usage": usage_json(12, 0)
            }
        }),
    );
    tools(&mut body);
    append_sse(
        &mut body,
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": null},
            "usage": usage_json(12, 4)
        }),
    );
    append_sse(&mut body, "message_stop", &json!({"type": "message_stop"}));
    body
}

/// One `bash` call running exactly `command` — the test's own, so `json!`
/// does the escaping.
fn bash_sse(command: &str) -> String {
    let input = json!({"command": command, "timeout": 10_000}).to_string();
    tool_message_sse("msg_bash_command_tools", |body| {
        append_tool_use(body, 0, "toolu_bash_command_e2e", "bash", &input);
    })
}

fn background_bash_sse(command: &str) -> String {
    let input = json!({"command": command, "run_in_background": true}).to_string();
    tool_message_sse("msg_background_bash_tools", |body| {
        append_tool_use(body, 0, "toolu_background_bash_e2e", "bash", &input);
    })
}

fn bash_read_sse() -> String {
    tool_message_sse("msg_bash_read_tools", |body| {
        append_tool_use(
            body,
            0,
            "toolu_bash_e2e",
            "bash",
            r#"{"command":"printf 'bash deterministic'","timeout":1000}"#,
        );
        append_tool_use(
            body,
            1,
            "toolu_read_e2e",
            "read_file",
            r#"{"path":"fixture.txt"}"#,
        );
    })
}

/// One delegation, with the fields the cell is supposed to read: a
/// `description` that says what the child is for and a `subagent_type` that
/// says which harness. Everything else in the input is noise the cell used to
/// print instead.
fn spawn_sse() -> String {
    tool_message_sse("msg_spawn_tools", |body| {
        append_tool_use(
            body,
            0,
            "toolu_spawn_e2e",
            "Agent",
            r#"{"allow_cross_provider":true,"description":"survey the fixture tree","subagent_type":"general-purpose","prompt":"look around and report"}"#,
        );
    })
}

fn workflow_two_helpers_response(request_index: usize, request: &str, final_text: &str) -> String {
    if request_contains_tool(request, "Workflow") {
        return text_sse("msg_workflow_parent_final", final_text);
    }
    for (marker, id) in [
        ("WORKFLOW_HELPER_ALPHA", "alpha"),
        ("WORKFLOW_HELPER_BETA", "beta"),
    ] {
        if request.contains(marker) {
            return if request_contains_tool(request, "bash") {
                text_sse(
                    &format!("msg_workflow_{id}_final"),
                    &format!("{marker} finished\n"),
                )
            } else {
                bash_sse_for(
                    &format!("toolu_workflow_{id}"),
                    &format!("sleep 3 && printf '{id} done'"),
                )
            };
        }
    }
    if request_index == 0 {
        return workflow_two_helpers_sse();
    }
    text_sse("msg_workflow_fallback", final_text)
}

/// The lane prompts the scripted decomposition hands out, and the findings
/// each lane answers with; the test asserts the findings reach the main turn.
pub const PRELUDE_LANES: [(&str, &str); 2] = [
    ("PRELUDE_LANE_ALPHA", "ALPHA_FINDING: the parser owns the alpha path"),
    ("PRELUDE_LANE_BETA", "BETA_FINDING: the lexer owns the beta path"),
];

fn host_prelude_response(request: &str, final_text: &str) -> String {
    // Stage 2 — the decomposition classifier: one `StructuredOutput` call,
    // then a closing line once its own result comes back.
    if request.contains("Split the user's task into AT MOST") {
        return if request_contains_tool(request, "StructuredOutput") {
            text_sse("msg_prelude_decompose_done", "split recorded\n")
        } else {
            let subtasks = json!({
                "subtasks": PRELUDE_LANES
                    .iter()
                    .map(|(marker, _)| json!({
                        "role": format!("{} lane", marker.rsplit('_').next().unwrap_or("x").to_ascii_lowercase()),
                        "prompt": format!("{marker}: report this lane's finding"),
                    }))
                    .collect::<Vec<_>>()
            });
            structured_output_sse("toolu_prelude_decompose", &subtasks.to_string())
        };
    }
    // Stage 3 — each lane answers its finding at once.
    for (marker, finding) in PRELUDE_LANES {
        if request.contains(marker) {
            return text_sse(
                &format!("msg_prelude_{}", marker.to_ascii_lowercase()),
                &format!("{finding}\n"),
            );
        }
    }
    // The main turn carries the seated pre-analysis (as a SpawnMultiAgent
    // result once the transcript has a user side to follow, inline on the
    // very first turn of a session).
    if request.contains("[Smart pre-analysis]") {
        return text_sse("msg_prelude_final", final_text);
    }
    text_sse(
        "msg_prelude_unexpected",
        "UNEXPECTED: the main turn ran without a pre-analysis\n",
    )
}

/// One `StructuredOutput` call carrying `input` — how a classifier agent
/// answers on the Anthropic path.
fn structured_output_sse(id: &str, input: &str) -> String {
    tool_message_sse(&format!("msg_{id}"), |body| {
        append_tool_use(body, 0, id, "StructuredOutput", input);
    })
}

fn workflow_two_helpers_sse() -> String {
    let input = json!({
        "name": "two helper e2e",
        "phases": [{
            "id": "inspect",
            "fanout": ["ALPHA", "BETA"],
            "prompt": "WORKFLOW_HELPER_{item}",
            "model": "claude-sonnet-4-6"
        }]
    })
    .to_string();
    tool_message_sse("msg_workflow_tools", |body| {
        append_tool_use(body, 0, "toolu_workflow_e2e", "Workflow", &input);
    })
}

fn bash_sse_for(id: &str, command: &str) -> String {
    let input = json!({"command": command, "timeout": 10_000}).to_string();
    tool_message_sse(&format!("msg_{id}"), |body| {
        append_tool_use(body, 0, id, "bash", &input);
    })
}

fn request_contains_tool(request: &str, tool: &str) -> bool {
    fn contains(value: &Value, tool: &str) -> bool {
        match value {
            Value::Object(object) => {
                (object.get("type").and_then(Value::as_str) == Some("tool_use")
                    && object.get("name").and_then(Value::as_str) == Some(tool))
                    || object.values().any(|value| contains(value, tool))
            }
            Value::Array(values) => values.iter().any(|value| contains(value, tool)),
            _ => false,
        }
    }

    serde_json::from_str::<Value>(request)
        .ok()
        .is_some_and(|value| contains(&value, tool))
}

/// A tool call that keeps the turn busy for seconds, so a test can type into a
/// running turn the way a person does.
fn slow_tool_sse() -> String {
    tool_message_sse("msg_slow_tools", |body| {
        append_tool_use(
            body,
            0,
            "toolu_slow_e2e",
            "bash",
            r#"{"command":"sleep 3 && printf 'slow done'","timeout":20000}"#,
        );
    })
}

/// A tool call nobody is meant to see finish. `sleep <seconds>` parks the turn
/// between the persisted `tool_use` and any result, which is the exact state a
/// killed zo leaves in the transcript.
fn parked_tool_sse(seconds: u32) -> String {
    tool_message_sse("msg_parked_tools", |body| {
        append_tool_use(
            body,
            0,
            "toolu_parked_e2e",
            "bash",
            &format!(
                r#"{{"command":"sleep {seconds} && printf 'parked done'","timeout":{}}}"#,
                u64::from(seconds) * 1_000 + 10_000
            ),
        );
    })
}

fn append_tool_use(buffer: &mut String, index: usize, id: &str, name: &str, input: &str) {
    append_sse(
        buffer,
        "content_block_start",
        &json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}
        }),
    );
    append_sse(
        buffer,
        "content_block_delta",
        &json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "input_json_delta", "partial_json": input}
        }),
    );
    append_sse(
        buffer,
        "content_block_stop",
        &json!({"type": "content_block_stop", "index": index}),
    );
}

fn append_sse(buffer: &mut String, event: &str, payload: &Value) {
    use std::fmt::Write as _;
    writeln!(buffer, "event: {event}").expect("event write should succeed");
    writeln!(buffer, "data: {payload}").expect("payload write should succeed");
    buffer.push('\n');
}

fn usage_json(input_tokens: u32, output_tokens: u32) -> Value {
    json!({
        "input_tokens": input_tokens,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
        "output_tokens": output_tokens
    })
}

fn baseline_response(task: &str, root: &str, patch: &str, request: &str) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let call = |id: &str, name: &str, input: Value| {
        tool_message_sse(id, |body| append_tool_use(body, 0, id, name, &input.to_string()))
    };
    if task == "Q2" {
        if request_contains_tool(request, "Agent") {
            if request.contains("BASELINE_CHILD_BETA") {
                let parsed: Value = serde_json::from_str(request).unwrap();
                let mut receipts = Vec::new();
                for message in parsed["messages"].as_array().into_iter().flatten() {
                    for block in message["content"].as_array().into_iter().flatten() {
                        if block["type"] == "tool_result" {
                            if let Some(text) = block["content"][0]["text"].as_str() {
                                if let Ok(value) = serde_json::from_str::<Value>(text) {
                                    if value["status"] == "completed" {
                                        if let Some(id) = value["agentId"].as_str() { receipts.push(id.to_owned()); }
                                    }
                                }
                            }
                        }
                    }
                }
                return text_sse("baseline_done", &format!("BASELINE_DONE: {}", receipts.join(", ")));
            }
            return call("BASELINE_CHILD_BETA", "Agent", json!({"description":"BASELINE_CHILD_BETA","subagent_type":"general-purpose","prompt":"BASELINE_CHILD_BETA: fix your assigned file","model":"claude-sonnet-4-6","background":false}));
        }
        for (marker, path, source) in [
            ("BASELINE_CHILD_ALPHA", "js/alpha.mjs", "export const double = (n) => n * 2;\n"),
            ("BASELINE_CHILD_BETA", "js/beta.mjs", "export const isEven = (n) => n % 2 === 0;\n"),
        ] {
            if request.contains(marker) {
                if !request_contains_tool(request, "read_file") {
                    return call(&format!("{marker}_READ"), "read_file", json!({"path":format!("{root}/repo/{path}")}));
                }
                return if request_contains_tool(request, "write_file") {
                    text_sse(marker, &format!("{marker} completed {path}"))
                } else {
                    call(&format!("{marker}_WRITE"), "write_file", json!({"path":format!("{root}/repo/{path}"),"content":source}))
                };
            }
        }
        return call("BASELINE_CHILD_ALPHA", "Agent", json!({"description":"BASELINE_CHILD_ALPHA","subagent_type":"general-purpose","prompt":"BASELINE_CHILD_ALPHA: fix your assigned file","model":"claude-sonnet-4-6","background":false}));
    }
    if task == "Q4" && !request_contains_tool(request, "EnterWorktree") {
        return call("baseline_enter", "EnterWorktree", json!({"path":format!("{root}/worktree"),"branch":"baseline-fix"}));
    }
    if !request.contains("baseline_fix") {
        let mut command = format!("git apply {}", quote(patch));
        if task == "Q4" { command.push_str(" && git add src/lib.rs && git -c commit.gpgsign=false commit -m 'Fix inclusive range'"); }
        if task == "Q6" {
            let helper = std::path::Path::new(patch).parent().unwrap().parent().unwrap().join("window-tools.mjs");
            let _ = write!(command, " && node {} handoff {}", quote(&helper.to_string_lossy()), quote(root));
        }
        return call("baseline_fix", "bash", json!({"command":command,"timeout":30000}));
    }
    text_sse("baseline_done", "BASELINE_DONE")
}

/* ---- the fork scenario (t-2875) ---- */

/// The question both children are asked. It sits in a USER text block only
/// in a child's transcript — the parent carries it inside a `tool_use` input.
#[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
pub const FORK_QUESTION: &str =
    "FORK_QUESTION: what is the marker line in fixture.txt? Answer with the marker only.";
/// The line the parent's `read_file` brings into its context.
#[allow(dead_code)]
pub const FORK_FIXTURE_MARKER: &str = "FIXTURE_MARKER_FOR_FORK";
/// The parent's own `read_file` call.
#[allow(dead_code)]
pub const PARENT_READ_ID: &str = "toolu_parent_read_e2e";
/// The parent's `Agent{subagent_type: "fork"}` call.
#[allow(dead_code)]
pub const FORK_SPAWN_ID: &str = "toolu_fork_spawn_e2e";
/// The parent's plain `Agent{subagent_type: "general-purpose"}` call.
#[allow(dead_code)]
pub const PLAIN_SPAWN_ID: &str = "toolu_plain_spawn_e2e";
/// A child's `read_file` — the road a child takes when its context does not
/// already hold the marker.
#[allow(dead_code)]
pub const CHILD_READ_ID: &str = "toolu_child_read_e2e";
/// The answer of a child whose context already held the marker (no tool).
#[allow(dead_code)]
pub const FORK_ANSWER_FROM_CONTEXT: &str = "FORK_ANSWER_FROM_CONTEXT";
/// The answer of a child that had to read the file first.
#[allow(dead_code)]
pub const FORK_ANSWER_AFTER_READ: &str = "FORK_ANSWER_AFTER_READ";
/// The `model` the fork call names — and a fork must ignore (CC contract:
/// a fork runs on the parent's model). Same family as the parent, so the
/// scenario proves "ignored", not "refused".
#[allow(dead_code)]
pub const FORK_IGNORED_MODEL: &str = "claude-haiku-4-5";

fn fork_inherits_response(request: &str, final_text: &str) -> String {
    let Ok(body) = serde_json::from_str::<Value>(request) else {
        return text_sse("msg_fork_bad_request", "UNEXPECTED: the request body is not JSON\n");
    };
    if user_text_contains(&body, FORK_QUESTION) {
        // A child's turn. One that already read answers; one whose context
        // holds the marker answers without a tool; the rest read first.
        if has_tool_result_id(&body, CHILD_READ_ID) {
            return text_sse("msg_child_after_read", &format!("{FORK_ANSWER_AFTER_READ}\n"));
        }
        if body_text_contains(&body, FORK_FIXTURE_MARKER) {
            return text_sse("msg_fork_from_context", &format!("{FORK_ANSWER_FROM_CONTEXT}\n"));
        }
        return read_fixture_sse("msg_child_reads", CHILD_READ_ID);
    }
    // The parent: read, fork, delegate plainly, close.
    if !has_tool_result_id(&body, PARENT_READ_ID) {
        return read_fixture_sse("msg_parent_reads", PARENT_READ_ID);
    }
    if !has_tool_result_id(&body, FORK_SPAWN_ID) {
        return agent_spawn_sse("msg_parent_forks", FORK_SPAWN_ID, "fork", Some(FORK_IGNORED_MODEL));
    }
    if !has_tool_result_id(&body, PLAIN_SPAWN_ID) {
        return agent_spawn_sse("msg_parent_delegates", PLAIN_SPAWN_ID, "general-purpose", None);
    }
    text_sse("msg_fork_scenario_final", final_text)
}

/// One `read_file` of the workspace fixture under `tool_use_id`.
fn read_fixture_sse(message_id: &str, tool_use_id: &str) -> String {
    tool_message_sse(message_id, |body| {
        append_tool_use(body, 0, tool_use_id, "read_file", r#"{"path":"fixture.txt"}"#);
    })
}

/// One blocking `Agent` call asking [`FORK_QUESTION`] of `subagent_type`,
/// naming `model` when the scenario wants an override on the wire.
fn agent_spawn_sse(
    message_id: &str,
    tool_use_id: &str,
    subagent_type: &str,
    model: Option<&str>,
) -> String {
    let mut input = json!({
        "description": "answer the fixture question",
        "subagent_type": subagent_type,
        "prompt": FORK_QUESTION,
        "background": false,
    });
    if let Some(model) = model {
        input["model"] = json!(model);
    }
    let input = input.to_string();
    tool_message_sse(message_id, |body| {
        append_tool_use(body, 0, tool_use_id, "Agent", &input);
    })
}

/// The `messages` of an Anthropic request body.
fn messages_of(body: &Value) -> &[Value] {
    body["messages"].as_array().map_or(&[], Vec::as_slice)
}

/// Whether any USER message carries `needle` in a plain text block (or a
/// bare string content). A `tool_use` input or a `tool_result` body does not
/// count — that is what tells a child's transcript from its parent's.
#[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
pub fn user_text_contains(body: &Value, needle: &str) -> bool {
    messages_of(body)
        .iter()
        .filter(|message| message["role"] == "user")
        .any(|message| match &message["content"] {
            Value::String(text) => text.contains(needle),
            Value::Array(blocks) => blocks.iter().any(|block| {
                block["type"] == "text" && block["text"].as_str().is_some_and(|text| text.contains(needle))
            }),
            _ => false,
        })
}

/// Whether the transcript already carries a `tool_result` for `tool_use_id`.
#[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
pub fn has_tool_result_id(body: &Value, tool_use_id: &str) -> bool {
    messages_of(body).iter().any(|message| {
        message["content"].as_array().is_some_and(|blocks| {
            blocks
                .iter()
                .any(|block| block["type"] == "tool_result" && block["tool_use_id"] == tool_use_id)
        })
    })
}

/// Whether `needle` appears in any text the model was shown in `messages`:
/// text blocks, and `tool_result` bodies (a string or text blocks).
#[allow(dead_code)] // e2e_hermetic only; the module is shared by every e2e binary.
pub fn body_text_contains(body: &Value, needle: &str) -> bool {
    fn text_of(value: &Value) -> Vec<&str> {
        match value {
            Value::String(text) => vec![text.as_str()],
            Value::Array(blocks) => blocks.iter().flat_map(text_of).collect(),
            Value::Object(object) => {
                let mut found: Vec<&str> = object.get("text").and_then(Value::as_str).into_iter().collect();
                if let Some(content) = object.get("content") {
                    found.extend(text_of(content));
                }
                found
            }
            _ => Vec::new(),
        }
    }
    messages_of(body)
        .iter()
        .flat_map(|message| text_of(&message["content"]))
        .any(|text| text.contains(needle))
}
