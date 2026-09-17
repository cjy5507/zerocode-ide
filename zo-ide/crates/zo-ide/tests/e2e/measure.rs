//! A stateful loopback provider for the r23 performance measurements.
//!
//! [`super::scripted::ScriptedAnthropicService`] answers by *request ordinal*,
//! which is exactly wrong for the three things r23 has to drive: a long session
//! whose turns repeat, a compaction round that interleaves a summary request
//! among them, and sub-agents whose own requests arrive concurrently with the
//! parent's. This server answers by **what the request contains** instead, so
//! order and concurrency stop mattering:
//!
//! | the request's last message | the answer |
//! |---|---|
//! | carries the compaction prompt | an 8-section summary |
//! | is a `tool_result` | the final text of that turn |
//! | mentions `R23_SPAWN` | `children` × `Agent`, run in the foreground |
//! | mentions `R23_TOOLS` | `tools` × `bash`, in one message |
//! | mentions `R23_BIG` | one `bash` whose output is `payload_lines` long |
//! | mentions `R23_SLEEP` | one `bash` that sleeps `child_sleep_secs` |
//! | anything else | the final text |
//!
//! Every recorded body is kept with its arrival instant, because half of what
//! r23 asks is not about the screen at all — it is about what compaction does
//! to the *bytes on the wire*.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

/// The marker the compaction system prompt opens with. Matching the real
/// constant's first sentence (rather than a shape guess) is what keeps a
/// summary request from being mistaken for an ordinary turn — which would end
/// the measurement with "compaction never fired" and no way to tell that apart
/// from a threshold that was set too high.
const COMPACTION_MARKER: &str = "You are summarizing a coding conversation";

/// The text every scripted turn ends with, so the PTY driver can wait for the
/// turn to settle without matching anything the composer echoed.
pub const TURN_SETTLED: &str = "r23-turn-settled";

/// A CHILD's final text. Distinct from [`TURN_SETTLED`] because a child's
/// report is rendered into the parent's transcript: sharing one marker made
/// the driver stop the clock on the first child and report three serial
/// children as one 3-second turn.
pub const CHILD_SETTLED: &str = "r23-child-settled";

/// What this server should do with a turn request.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    /// `bash` calls emitted in ONE assistant message for an `R23_TOOLS` turn.
    pub tools: usize,
    /// `Agent` calls emitted in ONE assistant message for an `R23_SPAWN` turn.
    pub children: usize,
    /// Distinct lines a `R23_BIG` tool result carries. Distinct, because the
    /// context compressor collapses repeated runs — an 8k blob of the same
    /// line would arrive as three lines and the session would never grow.
    pub payload_lines: usize,
    /// Seconds an `R23_SPAWN` child's own tool sleeps, so three children
    /// overlap for long enough to be sampled.
    pub child_sleep_secs: u64,
    /// The `background` field on each spawned `Agent`.
    ///
    /// Not a detail: `false` blocks the parent's tool dispatch until the child
    /// finishes, and `Agent` is not in `is_concurrency_safe`, so a batch of
    /// blocking spawns runs one at a time. `true` is what the interactive
    /// session actually defaults to. The two answers are different by an
    /// order of magnitude, so the measurement has to name which it took.
    pub background: bool,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            tools: 8,
            children: 3,
            payload_lines: 90,
            child_sleep_secs: 3,
            background: false,
        }
    }
}

/// One recorded provider request.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// Milliseconds since the server started — a shared clock for lining
    /// requests up against the per-turn wall clock the driver keeps.
    pub at_ms: u128,
    /// The verbatim request body.
    pub body: String,
    /// Whether this was the compaction summary round-trip.
    pub compaction_summary: bool,
}

impl Recorded {
    /// The `messages` array, serialized exactly as it arrived.
    ///
    /// The prefix comparison this exists for is a byte question — a provider's
    /// cache matches serialized prefixes, not parsed trees — so it must keep
    /// the wire order rather than round-tripping through a map.
    #[must_use]
    pub fn messages_json(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|value| value.get("messages").cloned())
            .map(|messages| messages.to_string())
            .unwrap_or_default()
    }

    /// Whether this request's FIRST message carries `needle` — the cheapest
    /// way to tell a sub-agent's own conversation from the parent's.
    #[must_use]
    pub fn opens_with(&self, needle: &str) -> bool {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|value| {
                value
                    .get("messages")
                    .and_then(Value::as_array)
                    .and_then(|messages| messages.first())
                    .map(Value::to_string)
            })
            .is_some_and(|first| first.contains(needle))
    }

    #[must_use]
    pub fn message_count(&self) -> usize {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|value| {
                value
                    .get("messages")
                    .and_then(Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or_default()
    }
}

/// A deterministic loopback provider that answers by request CONTENT.
pub struct MeasureService {
    base_url: String,
    recorded: Arc<Mutex<Vec<Recorded>>>,
    shutdown: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<()>,
}

impl MeasureService {
    pub async fn start(plan: Plan) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let nonces = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let state = Arc::clone(&recorded);
        let started = Instant::now();
        let join_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        let state = Arc::clone(&state);
                        let nonces = Arc::clone(&nonces);
                        tokio::spawn(async move {
                            let _ = serve(socket, state, nonces, plan, started).await;
                        });
                    }
                }
            }
        });
        Ok(Self {
            base_url: format!("http://{address}"),
            recorded,
            shutdown: Some(shutdown_tx),
            join_handle,
        })
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn recorded(&self) -> Vec<Recorded> {
        self.recorded.lock().await.clone()
    }
}

impl Drop for MeasureService {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.join_handle.abort();
    }
}

async fn serve(
    mut socket: TcpStream,
    recorded: Arc<Mutex<Vec<Recorded>>>,
    nonces: Arc<AtomicUsize>,
    plan: Plan,
    started: Instant,
) -> io::Result<()> {
    let (method, body) = read_http_request(&mut socket).await?;
    if !method.eq_ignore_ascii_case("POST") {
        socket
            .write_all(http_response("text/plain", "").as_bytes())
            .await?;
        return Ok(());
    }
    let answer = answer_for(&body, plan);
    let nonce = nonces.fetch_add(1, Ordering::Relaxed);
    let input_tokens = approximate_input_tokens(&body);
    if std::env::var_os("MEASURE_TRACE").is_some() {
        eprintln!(
            "[r23-wire] {}ms answer={answer:?} body={}B messages={}",
            started.elapsed().as_millis(),
            body.len(),
            serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| value
                    .get("messages")
                    .and_then(Value::as_array)
                    .map(Vec::len))
                .unwrap_or_default()
        );
    }
    recorded.lock().await.push(Recorded {
        at_ms: started.elapsed().as_millis(),
        body,
        compaction_summary: matches!(answer, Answer::Summary),
    });
    socket
        .write_all(
            http_response(
                "text/event-stream",
                &answer.sse(plan, nonce, input_tokens),
            )
            .as_bytes(),
        )
        .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Summary,
    Tools,
    Spawn,
    Big,
    Sleep,
    ChildText,
    Text,
}

impl Answer {
    fn sse(self, plan: Plan, nonce: usize, input_tokens: u32) -> String {
        match self {
            Self::Summary => text_sse("msg_r23_summary", &summary_body(), input_tokens),
            Self::Tools => tools_sse(plan.tools, input_tokens),
            Self::Spawn => spawn_sse(plan, input_tokens),
            Self::Big => big_output_sse(plan.payload_lines, nonce, input_tokens),
            Self::Sleep => sleep_sse(plan.child_sleep_secs, input_tokens),
            Self::ChildText => text_sse(
                "msg_r23_child",
                &format!("### Done\n\n- {CHILD_SETTLED}\n"),
                input_tokens,
            ),
            Self::Text => text_sse(
                "msg_r23_text",
                &format!("### Done\n\n- {TURN_SETTLED}\n"),
                input_tokens,
            ),
        }
    }
}

/// What a provider would report as this request's input size.
///
/// A mock that always answers `input_tokens: 12` is not merely imprecise — the
/// live footer reads the provider's number (`RenderBlock::Usage.ctx_tokens`,
/// deliberately NOT the local estimate), so a constant makes the screen read
/// `100% context left` for a session of any length and makes
/// `effective_context_tokens` fall back to the estimate alone. Four characters
/// per token over the whole serialized request is the same conversion the
/// runtime's own estimator uses, applied to the same bytes the provider sees.
fn approximate_input_tokens(body: &str) -> u32 {
    u32::try_from(body.len().div_ceil(4)).unwrap_or(u32::MAX)
}

/// Decide the answer from the request body alone.
///
/// Deliberately reads only the LAST message: the trigger words stay in the
/// transcript forever once typed, so a whole-body search would keep re-firing
/// the first turn's tool burst on every later turn of the same session.
fn answer_for(body: &str, _plan: Plan) -> Answer {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Answer::Text;
    };
    let Some(last) = value
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
    else {
        return Answer::Text;
    };
    let child = value
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.first())
        .is_some_and(|first| first.to_string().contains("R23_SLEEP"));
    let blocks = last.get("content").and_then(Value::as_array);
    if blocks.is_some_and(|blocks| {
        blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
    }) {
        return if child { Answer::ChildText } else { Answer::Text };
    }
    let text = last.to_string();
    if text.contains(COMPACTION_MARKER) {
        return Answer::Summary;
    }
    if text.contains("R23_SPAWN") {
        return Answer::Spawn;
    }
    if text.contains("R23_TOOLS") {
        return Answer::Tools;
    }
    if text.contains("R23_BIG") {
        return Answer::Big;
    }
    if text.contains("R23_SLEEP") {
        return Answer::Sleep;
    }
    Answer::Text
}

/// An eight-section summary with no backtick spans and no path-like tokens.
///
/// Both omissions are load-bearing: `summary_fabricates_identifiers` rejects a
/// summary whose cited identifiers are mostly ungrounded, and a rejected
/// summary silently falls back to the local extractor — the measurement would
/// then be of a code path the real provider never takes.
fn summary_body() -> String {
    "<analysis>\nThe session repeated one measured turn.\n</analysis>\n\n<summary>\n\
     1. Primary Request and Intent: repeat one scripted measurement turn until compaction fires.\n\
     2. Key Technical Concepts: repeated turns, a scripted provider, a growing transcript.\n\
     3. Files and Code Sections: none were edited during this session.\n\
     4. Errors and fixes: none occurred.\n\
     5. Problem Solving: the session simply repeated the same request.\n\
     6. All user messages: the user asked for the same measured turn each time.\n\
     7. Pending Tasks: continue repeating the measured turn.\n\
     8. Current Work: repeating the measured turn.\n</summary>\n"
        .to_string()
}

fn tools_sse(tools: usize, input_tokens: u32) -> String {
    let mut body = message_start("msg_r23_tools", input_tokens);
    for index in 0..tools {
        append_tool_use(
            &mut body,
            index,
            &format!("toolu_r23_tool_{index}"),
            "bash",
            &format!(r#"{{"command":"printf 'r23 probe {index}'","timeout":5000}}"#),
        );
    }
    finish_tool_message(&mut body, input_tokens);
    body
}

fn spawn_sse(plan: Plan, input_tokens: u32) -> String {
    let Plan {
        children,
        child_sleep_secs: sleep_secs,
        background,
        ..
    } = plan;
    let mut body = message_start("msg_r23_spawn", input_tokens);
    for index in 0..children {
        append_tool_use(
            &mut body,
            index,
            &format!("toolu_r23_child_{index}"),
            "Agent",
            &format!(
                r#"{{"description":"r23 child {index}","subagent_type":"general-purpose","background":{background},"prompt":"R23_SLEEP for {sleep_secs} seconds and report"}}"#
            ),
        );
    }
    finish_tool_message(&mut body, input_tokens);
    body
}

/// One tool call that occupies a child for a known, overlapping window.
///
/// `sleep` and not a busy loop on purpose: the question is whether three
/// children RUN at once, and a sleeping child answers it without adding CPU
/// that would then have to be subtracted from the parent's profile.
fn sleep_sse(seconds: u64, input_tokens: u32) -> String {
    let mut body = message_start("msg_r23_sleep", input_tokens);
    append_tool_use(
        &mut body,
        0,
        "toolu_r23_sleep",
        "bash",
        &json!({"command": format!("sleep {seconds} && printf 'r23 child awake'"), "timeout": 60000})
            .to_string(),
    );
    finish_tool_message(&mut body, input_tokens);
    body
}

/// One big tool result, stamped with the request's ordinal.
///
/// The stamp is not decoration. A full compaction can land mid-turn and leave
/// the preserved tail ending at the user's prompt with the `tool_use` that
/// answered it summarized away — so the runtime asks again, and a mock that
/// answers by content answers identically. zo then (correctly) stops the turn
/// with its tool-repetition guard: "`bash` has been called with identical
/// input 4 times without making progress". Stamping the command makes every
/// call distinct, so the measurement observes compaction instead of the guard.
fn big_output_sse(payload_lines: usize, nonce: usize, input_tokens: u32) -> String {
    let mut body = message_start("msg_r23_big", input_tokens);
    // `seq` output is distinct per line and long enough per line to move the
    // estimate, and `awk` keeps it one process instead of a shell loop whose
    // own start-up would dominate the tool's wall clock.
    let command = format!(
        "seq 1 {payload_lines} | awk '{{printf \"r23 payload {nonce} row %s carries a deterministic sentence of filler so the transcript grows\\n\", $1}}'"
    );
    append_tool_use(
        &mut body,
        0,
        "toolu_r23_big",
        "bash",
        &json!({"command": command, "timeout": 20000}).to_string(),
    );
    finish_tool_message(&mut body, input_tokens);
    body
}

fn message_start(id: &str, input_tokens: u32) -> String {
    let mut body = String::new();
    append_sse(
        &mut body,
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": "claude-sonnet-4-6",
                "stop_reason": null,
                "stop_sequence": null,
                "usage": usage_json(input_tokens, 0)
            }
        }),
    );
    body
}

fn finish_tool_message(body: &mut String, input_tokens: u32) {
    append_sse(
        body,
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": null},
            "usage": usage_json(input_tokens, 4)
        }),
    );
    append_sse(body, "message_stop", &json!({"type": "message_stop"}));
}

fn text_sse(message_id: &str, text: &str, input_tokens: u32) -> String {
    let mut body = message_start(message_id, input_tokens);
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
            "usage": usage_json(input_tokens, 8)
        }),
    );
    append_sse(&mut body, "message_stop", &json!({"type": "message_stop"}));
    body
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

fn http_response(content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

async fn read_http_request(socket: &mut TcpStream) -> io::Result<(String, String)> {
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request closed before headers",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
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

/// Length of the longest shared PREFIX of two serialized message arrays.
///
/// This is the only honest cache question a mock can answer. A mock invents
/// its own `cache_creation` / `cache_read` numbers, so reading them back would
/// be circular; what a provider actually matches is the serialized prefix, and
/// that is real in every request the driver captured.
#[must_use]
pub fn shared_prefix_len(left: &str, right: &str) -> usize {
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .take_while(|(left, right)| left == right)
        .count()
}

#[cfg(test)]
mod tests {
    use super::{answer_for, shared_prefix_len, Answer, Plan};

    #[test]
    fn only_the_last_message_decides_the_answer() {
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "R23_TOOLS go"}]},
                {"role": "user", "content": [{"type": "text", "text": "say something"}]}
            ]
        })
        .to_string();
        assert_eq!(answer_for(&body, Plan::default()), Answer::Text);
    }

    #[test]
    fn a_tool_result_ends_the_turn() {
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": "x"}]}
            ]
        })
        .to_string();
        assert_eq!(answer_for(&body, Plan::default()), Answer::Text);
    }

    #[test]
    fn the_compaction_prompt_is_recognized() {
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "You are summarizing a coding conversation that is about to run out of context window."}]}
            ]
        })
        .to_string();
        assert_eq!(answer_for(&body, Plan::default()), Answer::Summary);
    }

    #[test]
    fn shared_prefix_counts_bytes_not_elements() {
        assert_eq!(shared_prefix_len("[1,2,3]", "[1,2,9]"), 5);
        assert_eq!(shared_prefix_len("[]", "[1]"), 1);
    }
}
