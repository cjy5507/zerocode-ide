//! `zo attach` — a thin client for a running [`zo serve`](crate::serve).
//!
//! Connects to the session server over TCP, optionally creates a session,
//! replays its history, then drops into a line REPL: each line you type is a
//! `session.run_turn`, whose [`RenderBlock`](runtime::message_stream) frames
//! stream back and render live. Ctrl-D detaches — the session keeps living on
//! the server, ready for the next `zo attach`.
//!
//! This is the proof-of-concept client: a single-connection, line-oriented terminal rather
//! than the full ratatui `App`. It speaks the exact same wire protocol the TUI
//! attach would (see [`crate::serve_protocol`]), so promoting it to a rich
//! client later is additive — the server contract does not change.
//!
//! ## Frame interpretation
//!
//! Server lines are either JSON-RPC responses (terminate a request) or render
//! frames (the canonical `SerializableRenderBlock` JSON). The client tells them
//! apart structurally via [`is_response_line`](crate::serve_protocol::is_response_line)
//! and renders each frame by its `type` tag — the same vocabulary
//! `zo -p --output-format stream-json` emits.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use serde_json::Value as JsonValue;

use crate::serve_protocol::{is_response_line, RpcRequest};

/// ANSI dim escape, for muted secondary output (system notices, tool lines).
const DIM: &str = "\u{1b}[2m";
/// ANSI reset escape.
const RESET: &str = "\u{1b}[0m";

/// Entry point for `zo attach`. Builds a single-threaded runtime (the client
/// only does socket + stdin I/O — no turn is driven locally) and connects.
///
/// `session_id` is `None` for `zo attach` with no id, which creates a fresh
/// session on the server and attaches to it; `Some(id)` attaches to an existing
/// session.
pub(crate) fn run_attach(
    bind_addr: String,
    session_id: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(attach_main(bind_addr, session_id))
}

/// Connect, resolve the session, replay history, then run the line REPL.
async fn attach_main(
    bind_addr: String,
    session_id: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = TcpStream::connect(&bind_addr).await.map_err(|error| {
        format!("zo attach: cannot connect to {bind_addr}: {error}. Is `zo serve` running?")
    })?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half).lines();
    let mut next_id: u64 = 1;
    // Shared secret for a guarded server (see `serve_auth`); `None` on the
    // tokenless loopback default, in which case nothing is stamped on the wire.
    let token = crate::serve_auth::token_from_env();
    let token = token.as_deref();

    // Resolve the session: create one if the user didn't name an existing id.
    let session_id = if let Some(id) = session_id {
        id
    } else {
        let result = request(
            &mut write_half,
            &mut reader,
            &mut next_id,
            "session.create",
            JsonValue::Null,
            token,
        )
        .await?;
        let id = result
            .get("id")
            .and_then(JsonValue::as_str)
            .ok_or("session.create: server did not return an id")?
            .to_string();
        println!("created session {id}");
        id
    };

    // Replay history so a reattach shows the prior conversation.
    let loaded = request(
        &mut write_half,
        &mut reader,
        &mut next_id,
        "session.load",
        serde_json::json!({ "id": session_id }),
        token,
    )
    .await
    .map_err(|error| format!("session.load failed: {error}"))?;
    print_history(&loaded);
    println!("{DIM}── attached to {session_id} · type a message, Ctrl-D to detach ──{RESET}");

    run_repl(
        &mut write_half,
        &mut reader,
        &mut next_id,
        &session_id,
        token,
    )
    .await?;

    println!("\n{DIM}detached — session {session_id} still alive on the server.{RESET}");
    Ok(())
}

/// The interactive loop: read stdin lines, drive each as a turn, render frames.
async fn run_repl(
    write_half: &mut OwnedWriteHalf,
    reader: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>,
    next_id: &mut u64,
    session_id: &str,
    token: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    while let Some(input) = stdin.next_line().await? {
        if input.trim().is_empty() {
            continue;
        }
        let id = *next_id;
        *next_id += 1;
        send_request(
            write_half,
            RpcRequest::new(
                id,
                "session.run_turn",
                serde_json::json!({ "id": session_id, "input": input }),
            ),
            token,
        )
        .await?;
        // Stream frames until the terminal response for this request arrives.
        loop {
            let Some(line) = reader.next_line().await? else {
                return Err("server closed the connection mid-turn".into());
            };
            let value: JsonValue = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if is_response_line(&value) {
                print_turn_result(&value);
                break;
            }
            render_frame(&value);
        }
    }
    Ok(())
}

/// Send a request and read lines until the matching response, returning its
/// `result` body. Used for non-streaming methods (`create`, `load`) — any stray
/// render frame seen before the response is ignored.
async fn request(
    write_half: &mut OwnedWriteHalf,
    reader: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>,
    next_id: &mut u64,
    method: &str,
    params: JsonValue,
    token: Option<&str>,
) -> Result<JsonValue, Box<dyn std::error::Error>> {
    let id = *next_id;
    *next_id += 1;
    send_request(write_half, RpcRequest::new(id, method, params), token).await?;
    loop {
        let Some(line) = reader.next_line().await? else {
            return Err("server closed the connection".into());
        };
        let value: JsonValue = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if !is_response_line(&value) {
            continue;
        }
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown error");
            return Err(format!("{method}: {message}").into());
        }
        return Ok(value.get("result").cloned().unwrap_or(JsonValue::Null));
    }
}

/// Serialize a request as one `\n`-terminated JSON line and write it, stamping
/// the shared-secret `token` (if any) so a guarded server accepts it.
async fn send_request(
    write_half: &mut OwnedWriteHalf,
    request: RpcRequest,
    token: Option<&str>,
) -> std::io::Result<()> {
    let request = request.with_token(token.map(str::to_owned));
    let mut line = serde_json::to_vec(&request).map_err(std::io::Error::other)?;
    line.push(b'\n');
    write_half.write_all(&line).await?;
    write_half.flush().await
}

/// Print the replayed conversation history from a `session.load` result.
fn print_history(loaded: &JsonValue) {
    let Some(entries) = loaded.get("history").and_then(JsonValue::as_array) else {
        return;
    };
    if entries.is_empty() {
        println!("{DIM}(no history yet){RESET}");
        return;
    }
    for entry in entries {
        let role = entry.get("role").and_then(JsonValue::as_str).unwrap_or("?");
        let text = entry.get("text").and_then(JsonValue::as_str).unwrap_or("");
        match role {
            "user" => println!("{DIM}you ›{RESET} {text}"),
            "assistant" => println!("{text}"),
            _ => println!("{DIM}{role}: {text}{RESET}"),
        }
    }
}

/// Render one streamed [`RenderBlock`](runtime::message_stream) frame to the
/// terminal, interpreting the canonical `SerializableRenderBlock` schema by its
/// `type` tag.
fn render_frame(frame: &JsonValue) {
    use std::io::Write;
    let kind = frame.get("type").and_then(JsonValue::as_str).unwrap_or("");
    match kind {
        "text_delta" => {
            if let Some(text) = frame.get("text").and_then(JsonValue::as_str) {
                print!("{text}");
            }
            if frame.get("done").and_then(JsonValue::as_bool) == Some(true) {
                println!();
            }
            let _ = std::io::stdout().flush();
        }
        "reasoning" => {
            if let Some(text) = frame.get("text").and_then(JsonValue::as_str) {
                print!("{DIM}{text}{RESET}");
                let _ = std::io::stdout().flush();
            }
        }
        "tool_call" => {
            let name = frame
                .get("name")
                .and_then(JsonValue::as_str)
                .unwrap_or("tool");
            let summary = frame
                .get("summary")
                .and_then(JsonValue::as_str)
                .unwrap_or("");
            println!("\n{DIM}⚙ {name}{RESET} {summary}");
        }
        "tool_result" => {
            let is_error = frame.get("is_error").and_then(JsonValue::as_bool) == Some(true);
            let content = frame
                .get("content")
                .and_then(JsonValue::as_str)
                .unwrap_or("");
            let preview = content.lines().next().unwrap_or("");
            let marker = if is_error { "✗" } else { "↳" };
            println!("{DIM}  {marker} {preview}{RESET}");
        }
        "system" => {
            if let Some(text) = frame.get("text").and_then(JsonValue::as_str) {
                println!("{DIM}· {text}{RESET}");
            }
        }
        "agent_result" => {
            // The collapsible card is a rich-TUI affordance; the line client
            // prints a compact header + the full body so a finished background
            // agent's result is never silently dropped on the plain path.
            let label = frame
                .get("label")
                .and_then(JsonValue::as_str)
                .unwrap_or("agent");
            let status = frame.get("status").and_then(JsonValue::as_str);
            let marker = if status == Some("failed") { "✗" } else { "✓" };
            let body = frame.get("body").and_then(JsonValue::as_str).unwrap_or("");
            println!("{DIM}⎔ agent · {label} {marker}{RESET}");
            if !body.is_empty() {
                println!("{body}");
            }
        }
        "permission_prompt" => {
            let tool = frame
                .get("tool_name")
                .and_then(JsonValue::as_str)
                .unwrap_or("tool");
            println!("{DIM}· permission prompt for {tool} (auto-denied in serve mode){RESET}");
        }
        // usage / rate_limit / image / user_question_prompt: not surfaced in the
        // proof-of-concept line client.
        _ => {}
    }
}

/// Print the terminal turn outcome (error, or a compact usage footer).
fn print_turn_result(response: &JsonValue) {
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(JsonValue::as_str)
            .unwrap_or("unknown error");
        println!("\n{DIM}✗ turn failed: {message}{RESET}");
        return;
    }
    let Some(result) = response.get("result") else {
        return;
    };
    let iterations = result
        .get("iterations")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let output = result
        .get("usage")
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    println!("{DIM}· {iterations} step(s) · {output} output tokens{RESET}");
}
