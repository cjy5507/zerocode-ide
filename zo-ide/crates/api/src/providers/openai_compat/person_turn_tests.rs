//! The person's turn of 2026-11-20 (t-121998) against a fake OpenAI that
//! answers as the real one did: an OpenAI API key, `gpt-6.2-astra` at effort
//! `max`, function tools. These tests use only the client's own surface, so
//! the same file runs on the code before the fix, where the first refusal
//! ends the turn; the helpers serve `responses_tests` as well.

use super::{MessageStream, OpenAiCompatClient, OpenAiCompatConfig};
use crate::error::ApiError;
use crate::types::{
    ContentBlockDelta, EffortLevel, InputContentBlock, InputMessage, MessageRequest,
    OutputContentBlock, StreamEvent, SystemBlock, ToolDefinition, ToolResultContentBlock,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// The first refusal on the person's screen, word for word.
pub(super) const EFFORT_REFUSAL: &str = "Unsupported value: 'reasoning_effort' does not support 'max' with this model. Supported values are: 'low', 'medium', 'high', and 'xhigh'.";
/// The second, once the effort was one the model takes.
pub(super) const TOOLS_REFUSAL: &str = "Function tools with reasoning_effort are not supported for gpt-6.2-astra in /v1/chat/completions. To use function tools, use /v1/responses or set reasoning_effort to 'none'.";

/// The model's catalog row as model discovery leaves it from a ChatGPT
/// subscription's model list: rungs up to `max` and `ultra`, which the API
/// does not take for this model.
pub(super) const SUBSCRIPTION_ROW: &str = r#"{"models":[{"provider":"openai","ids":["gpt-6.2-astra"],"context_window":272000,"effort_levels":["low","medium","high","xhigh","max","ultra"]}]}"#;

pub(super) const MODEL: &str = "gpt-6.2-astra";

struct EnvGuard {
    key: &'static str,
    before: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let before = std::env::var_os(key);
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
        Self { key, before }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.before.take() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

/// The catalog row in place for one test, under the crate's env lock. The
/// row is put back before the lock is let go (fields drop in order).
pub(super) struct RowInPlace {
    _row: EnvGuard,
    _lock: std::sync::MutexGuard<'static, ()>,
}

pub(super) fn with_subscription_row() -> RowInPlace {
    let lock = crate::test_env_lock();
    RowInPlace {
        _row: EnvGuard::set(crate::providers::MODEL_CONTEXT_WINDOWS_ENV, Some(SUBSCRIPTION_ROW)),
        _lock: lock,
    }
}

pub(super) fn bash_tool() -> ToolDefinition {
    ToolDefinition {
        name: "bash".to_string(),
        description: Some("Run a shell command".to_string()),
        input_schema: json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        }),
    }
}

pub(super) fn turn(messages: Vec<InputMessage>) -> MessageRequest {
    MessageRequest {
        model: MODEL.to_string(),
        max_tokens: 4096,
        messages,
        system: Some(vec![SystemBlock::Text {
            text: "Follow the house rules.".to_string(),
            cache_control: None,
        }]),
        tools: Some(vec![bash_tool()]),
        tool_choice: None,
        stream: true,
        thinking: None,
        output_config: None,
        effort: Some(EffortLevel::Max),
        effort_band_ceiling: None,
    }
}

pub(super) fn user(text: &str) -> InputMessage {
    InputMessage::user_text(text)
}

fn assistant_call(call_id: &str, command: &str, replay: Option<Value>) -> InputMessage {
    InputMessage {
        role: "assistant".to_string(),
        content: vec![InputContentBlock::ToolUse {
            id: call_id.to_string(),
            name: "bash".to_string(),
            input: json!({"command": command}),
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: replay,
    }
}

fn tool_result(call_id: &str, text: &str) -> InputMessage {
    InputMessage {
        role: "user".to_string(),
        content: vec![InputContentBlock::ToolResult {
            tool_use_id: call_id.to_string(),
            content: vec![ToolResultContentBlock::Text {
                text: text.to_string(),
            }],
            is_error: false,
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: None,
    }
}

/// One request the fake saw: where it went and what it carried.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    body: Value,
}

type Answer = Box<dyn Fn(&str, &Value, &[Seen]) -> (u16, &'static str, String) + Send + Sync>;

/// A fake HTTP endpoint on loopback: every request is kept, and `answer`
/// gives its status, content type and body.
async fn fake_endpoint(answer: Answer) -> (String, Arc<Mutex<Vec<Seen>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let seen = Arc::new(Mutex::new(Vec::<Seen>::new()));
    let kept = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut raw = Vec::new();
            let mut buffer = [0u8; 8192];
            let (head_end, length) = loop {
                let Ok(read) = socket.read(&mut buffer).await else {
                    break (None, 0);
                };
                if read == 0 {
                    break (None, 0);
                }
                raw.extend_from_slice(&buffer[..read]);
                if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (Some(end + 4), length);
                }
            };
            let Some(head_end) = head_end else {
                continue;
            };
            while raw.len() < head_end + length {
                match socket.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => raw.extend_from_slice(&buffer[..read]),
                }
            }
            let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
            let path = head
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            let body: Value =
                serde_json::from_slice(&raw[head_end..]).unwrap_or(Value::Null);
            let (status, content_type, reply) = {
                let mut seen = kept.lock().expect("seen");
                let reply = answer(&path, &body, &seen);
                seen.push(Seen { path, body });
                reply
            };
            let reason = if status == 200 { "OK" } else { "Bad Request" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://{addr}/v1"), seen)
}

fn openai_error(message: &str, param: &str) -> (u16, &'static str, String) {
    (
        400,
        "application/json",
        json!({"error": {"message": message, "type": "invalid_request_error", "param": param, "code": null}})
            .to_string(),
    )
}

pub(super) fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

pub(super) fn usage() -> Value {
    json!({"input_tokens": 120, "input_tokens_details": {"cached_tokens": 100}, "output_tokens": 30, "output_tokens_details": {"reasoning_tokens": 12}, "total_tokens": 150})
}

/// A `/responses` stream whose answer is one reasoning item and one
/// `bash` call, in the event shapes OpenAI's SDK declares.
fn tool_call_stream(call_id: &str) -> String {
    let reasoning = json!({"id": "rs_1", "type": "reasoning", "summary": [], "encrypted_content": "gAAAA-fake-reasoning"});
    let arguments = r#"{"command":"ls | wc -l"}"#;
    let call = json!({"id": "fc_1", "type": "function_call", "status": "completed", "call_id": call_id, "name": "bash", "arguments": arguments});
    sse(&[
        json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_1", "object": "response", "status": "in_progress", "model": MODEL, "output": []}}),
        json!({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0, "item": {"id": "rs_1", "type": "reasoning", "summary": []}}),
        json!({"type": "response.output_item.done", "sequence_number": 2, "output_index": 0, "item": reasoning}),
        json!({"type": "response.output_item.added", "sequence_number": 3, "output_index": 1, "item": {"id": "fc_1", "type": "function_call", "status": "in_progress", "call_id": call_id, "name": "bash", "arguments": ""}}),
        json!({"type": "response.function_call_arguments.delta", "sequence_number": 4, "item_id": "fc_1", "output_index": 1, "delta": arguments}),
        json!({"type": "response.function_call_arguments.done", "sequence_number": 5, "item_id": "fc_1", "output_index": 1, "arguments": arguments}),
        json!({"type": "response.output_item.done", "sequence_number": 6, "output_index": 1, "item": call}),
        json!({"type": "response.completed", "sequence_number": 7, "response": {"id": "resp_1", "object": "response", "status": "completed", "model": MODEL, "output": [reasoning, call], "usage": usage()}}),
    ])
}

/// A `/responses` stream whose answer is the text `3 files`.
fn answer_stream() -> String {
    let message = json!({"id": "msg_1", "type": "message", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "3 files", "annotations": []}]});
    sse(&[
        json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_2", "object": "response", "status": "in_progress", "model": MODEL, "output": []}}),
        json!({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0, "item": {"id": "msg_1", "type": "message", "role": "assistant", "status": "in_progress", "content": []}}),
        json!({"type": "response.content_part.added", "sequence_number": 2, "item_id": "msg_1", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}}),
        json!({"type": "response.output_text.delta", "sequence_number": 3, "item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": "3 files", "logprobs": []}),
        json!({"type": "response.output_text.done", "sequence_number": 4, "item_id": "msg_1", "output_index": 0, "content_index": 0, "text": "3 files", "logprobs": []}),
        json!({"type": "response.content_part.done", "sequence_number": 5, "item_id": "msg_1", "output_index": 0, "content_index": 0, "part": {"type": "output_text", "text": "3 files", "annotations": []}}),
        json!({"type": "response.output_item.done", "sequence_number": 6, "output_index": 0, "item": message}),
        json!({"type": "response.completed", "sequence_number": 7, "response": {"id": "resp_2", "object": "response", "status": "completed", "model": MODEL, "output": [message], "usage": usage()}}),
    ])
}

/// What OpenAI answered on 2026-11-20, for a fake that takes function tools
/// on `/v1/responses` only: an effort the model does not take is refused on
/// either wire, and Chat refuses tools with an effort.
fn like_openai(call_id: &'static str) -> Answer {
    Box::new(move |path, body, seen| match path {
        "/v1/chat/completions" => {
            if body["reasoning_effort"] == "max" {
                openai_error(EFFORT_REFUSAL, "reasoning_effort")
            } else if body.get("tools").is_some() && body.get("reasoning_effort").is_some() {
                openai_error(TOOLS_REFUSAL, "reasoning_effort")
            } else {
                openai_error("unexpected request on the fake's chat road", "")
            }
        }
        "/v1/responses" => {
            let effort = body.pointer("/reasoning/effort").and_then(Value::as_str);
            if !matches!(effort, None | Some("low" | "medium" | "high" | "xhigh")) {
                return openai_error(
                    "Unsupported value: 'reasoning.effort' does not support 'max' with this model. Supported values are: 'low', 'medium', 'high', and 'xhigh'.",
                    "reasoning.effort",
                );
            }
            let answered = seen.iter().filter(|earlier| earlier.path == "/v1/responses").count();
            let stream = if answered == 0 { tool_call_stream(call_id) } else { answer_stream() };
            (200, "text/event-stream", stream)
        }
        _ => openai_error("unknown path", ""),
    })
}

/// A client for a person's provider row at `base_url`, the way the window
/// writes one for an OpenAI API key.
fn client_at(base_url: &str) -> OpenAiCompatClient {
    OpenAiCompatClient::new(
        "test-key",
        OpenAiCompatConfig::from_user("OpenAI", base_url, Some("ZO_TEST_OPENAI_KEY"), true),
    )
    .with_base_url(base_url)
    .with_retry_policy(
        0,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_millis(2),
    )
}

async fn drain(mut stream: MessageStream) -> Result<Vec<StreamEvent>, ApiError> {
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await? {
        events.push(event);
    }
    Ok(events)
}

fn text_of(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ContentBlockDelta(delta) => match &delta.delta {
                ContentBlockDelta::TextDelta { text } => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn tool_calls_of(events: &[StreamEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ContentBlockStart(start) => match &start.content_block {
                OutputContentBlock::ToolUse { id, name, .. } => Some((id.clone(), name.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn arguments_of(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ContentBlockDelta(delta) => match &delta.delta {
                ContentBlockDelta::InputJsonDelta { partial_json } => Some(partial_json.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn replay_of(events: &[StreamEvent]) -> Option<Value> {
    events.iter().find_map(|event| match event {
        StreamEvent::MessageDelta(delta) => delta.delta.reasoning_replay.clone(),
        _ => None,
    })
}

fn effort_on(seen: &Seen) -> Option<&str> {
    seen.body
        .get("reasoning_effort")
        .or_else(|| seen.body.pointer("/reasoning/effort"))
        .and_then(Value::as_str)
}

/// The person's turn, end to end against a fake that answers as OpenAI did:
/// `max` is refused and goes again at `xhigh`, Chat's refusal of tools sends
/// the request to `/v1/responses`, the model calls `bash`, and the turn's
/// second step goes straight to `/v1/responses` at `xhigh` with the reasoning
/// item, the call and its output sent back. Before t-121998 the first refusal
/// ended the turn.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn the_person_s_two_refusals_end_in_a_tool_call_and_then_an_answer() {
    let _env = with_subscription_row();
    let (base_url, seen) = fake_endpoint(like_openai("call_t121998_a")).await;
    let client = client_at(&base_url);

    let first = drain(
        client
            .stream_message(&turn(vec![user("count the files here, one line")]))
            .await
            .expect("the first step is answered"),
    )
    .await
    .expect("the first step streams to its end");
    assert_eq!(
        tool_calls_of(&first),
        vec![("call_t121998_a".to_string(), "bash".to_string())]
    );
    assert_eq!(arguments_of(&first), r#"{"command":"ls | wc -l"}"#);
    {
        let seen = seen.lock().expect("seen");
        let route: Vec<(&str, Option<&str>)> = seen
            .iter()
            .map(|request| (request.path.as_str(), effort_on(request)))
            .collect();
        assert_eq!(
            route,
            vec![
                ("/v1/chat/completions", Some("max")),
                ("/v1/chat/completions", Some("xhigh")),
                ("/v1/responses", Some("xhigh")),
            ],
            "each refusal is answered once, in order"
        );
        let body = &seen[2].body;
        assert_eq!(body["model"], json!(MODEL));
        assert_eq!(body["store"], json!(false));
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["max_output_tokens"], json!(4096));
        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["name"], json!("bash"));
        assert!(body.get("service_tier").is_none(), "{body}");
        assert!(body.pointer("/reasoning/summary").is_none(), "{body}");
    }

    let replay = replay_of(&first);
    assert!(replay.is_some(), "the reasoning before the call comes back to be sent again");
    let second = drain(
        client
            .stream_message(&turn(vec![
                user("count the files here, one line"),
                assistant_call("call_t121998_a", "ls | wc -l", replay),
                tool_result("call_t121998_a", "3"),
            ]))
            .await
            .expect("the second step is answered"),
    )
    .await
    .expect("the second step streams to its end");
    assert_eq!(text_of(&second), "3 files");

    let seen = seen.lock().expect("seen");
    assert_eq!(seen.len(), 4, "the second step goes straight to /v1/responses: {seen:?}");
    assert_eq!(seen[3].path, "/v1/responses");
    assert_eq!(effort_on(&seen[3]), Some("xhigh"));
    let kinds: Vec<&str> = seen[3].body["input"]
        .as_array()
        .expect("input items")
        .iter()
        .map(|item| item["type"].as_str().unwrap_or("message"))
        .collect();
    assert_eq!(
        kinds,
        vec!["message", "reasoning", "function_call", "function_call_output"]
    );
    let input = &seen[3].body["input"];
    assert_eq!(input[1]["encrypted_content"], json!("gAAAA-fake-reasoning"));
    assert_eq!(input[2]["call_id"], json!("call_t121998_a"));
    assert_eq!(input[3]["call_id"], json!("call_t121998_a"));
    assert_eq!(input[3]["output"], json!("3"));
}

/// A refusal whose words are not one of the two is sent once and comes back
/// as it came: one request, the endpoint's own error.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn any_other_refusal_is_sent_once_and_comes_back_unchanged() {
    let _env = with_subscription_row();
    let (base_url, seen) = fake_endpoint(Box::new(|_, _, _| {
        openai_error("Invalid schema for function 'bash'.", "tools[0].parameters")
    }))
    .await;
    let error = client_at(&base_url)
        .stream_message(&turn(vec![user("hi")]))
        .await
        .expect_err("the refusal comes back");
    assert!(error.to_string().contains("Invalid schema for function 'bash'."), "{error}");
    assert_eq!(seen.lock().expect("seen").len(), 1);
}

/// An endpoint that goes on refusing after the effort it named was sent is
/// not asked a third time: the effort is sent again once per request.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn the_effort_is_sent_again_once_per_request() {
    let _env = with_subscription_row();
    let (base_url, seen) = fake_endpoint(Box::new(|_, _, _| {
        openai_error(EFFORT_REFUSAL, "reasoning_effort")
    }))
    .await;
    let error = client_at(&base_url)
        .stream_message(&turn(vec![user("hi")]))
        .await
        .expect_err("the second refusal comes back");
    assert!(error.to_string().contains("does not support 'max'"), "{error}");
    let seen = seen.lock().expect("seen");
    let efforts: Vec<Option<&str>> = seen.iter().map(effort_on).collect();
    assert_eq!(efforts, vec![Some("max"), Some("xhigh")]);
}

/// The non-streaming road (`send_message`) takes the same way and reads the
/// one-shot `/responses` answer.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_one_shot_request_reads_the_responses_answer() {
    let _env = with_subscription_row();
    let (base_url, seen) = fake_endpoint(Box::new(|path, body, _| match path {
        "/v1/chat/completions" if body["reasoning_effort"] == "max" => {
            openai_error(EFFORT_REFUSAL, "reasoning_effort")
        }
        "/v1/chat/completions" => openai_error(TOOLS_REFUSAL, "reasoning_effort"),
        _ => (
            200,
            "application/json",
            json!({"id": "resp_3", "object": "response", "status": "completed", "model": MODEL,
                "output": [{"id": "msg_3", "type": "message", "role": "assistant", "status": "completed",
                    "content": [{"type": "output_text", "text": "done", "annotations": []}]}],
                "usage": usage()})
            .to_string(),
        ),
    }))
    .await;
    let mut request = turn(vec![user("hi")]);
    request.stream = false;
    let response = client_at(&base_url)
        .send_message(&request)
        .await
        .expect("the one-shot answer");
    assert!(matches!(&response.content[..], [OutputContentBlock::Text { text }] if text == "done"));
    // A person's row is not taken to report cache reads: they stay inside
    // the input count, as on Chat.
    assert_eq!(response.usage.input_tokens, 120);
    assert_eq!(response.usage.cache_read_input_tokens, 0);
    let seen = seen.lock().expect("seen");
    assert_eq!(seen.last().map(|request| request.path.as_str()), Some("/v1/responses"));
    assert_eq!(seen.last().map(|request| request.body["stream"].clone()), Some(json!(false)));
}

/// A `/responses` stream that fails before any output (`response.failed`,
/// `server_error`) is opened again, and the turn ends in its answer.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_responses_stream_that_fails_before_any_output_is_opened_again() {
    let _env = with_subscription_row();
    let (base_url, seen) = fake_endpoint(Box::new(|path, body, seen| match path {
        "/v1/chat/completions" if body["reasoning_effort"] == "max" => {
            openai_error(EFFORT_REFUSAL, "reasoning_effort")
        }
        "/v1/chat/completions" => openai_error(TOOLS_REFUSAL, "reasoning_effort"),
        _ if seen.iter().any(|earlier| earlier.path == "/v1/responses") => {
            (200, "text/event-stream", answer_stream())
        }
        _ => (
            200,
            "text/event-stream",
            sse(&[
                json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_f", "status": "in_progress", "model": MODEL, "output": []}}),
                json!({"type": "response.failed", "sequence_number": 1, "response": {"id": "resp_f", "status": "failed", "model": MODEL, "output": [],
                    "error": {"code": "server_error", "message": "The server had an error while processing your request."}}}),
            ]),
        ),
    }))
    .await;
    let client = client_at(&base_url).with_retry_policy(
        1,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_millis(2),
    );
    let events = drain(
        client
            .stream_message(&turn(vec![user("hi")]))
            .await
            .expect("the step is answered"),
    )
    .await
    .expect("the stream recovers");
    assert_eq!(text_of(&events), "3 files");
    let paths: Vec<String> = seen.lock().expect("seen").iter().map(|request| request.path.clone()).collect();
    assert_eq!(
        paths,
        vec!["/v1/chat/completions", "/v1/chat/completions", "/v1/responses", "/v1/responses"]
    );
}
