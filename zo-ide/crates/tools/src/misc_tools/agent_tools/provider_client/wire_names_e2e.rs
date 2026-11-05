//! One task — read a file, fix it, run a command — through every provider zo
//! talks to (t-96186): the real request builder and conversation loop, each
//! provider's real client on a local fake server, and the real tools in a
//! temporary workspace. The model calls the general names zo shows it (`Read`
//! with `file_path`), or — as a resumed session or a model's habit still does —
//! the handlers' own (`read_file` with `path`); either way the task is done,
//! and no request ever shows the model a handler's own name or field.

use std::collections::BTreeSet;
use std::io::{Read as _, Write as _};
use std::sync::{Arc, Mutex};

use core_types::tool_names::{field_to_wire, CoreTool};
use runtime::{ContentBlock, ConversationRuntime, PermissionMode, PermissionPolicy, Session};
use serde_json::{json, Value};

use super::ProviderRuntimeClient;
use crate::misc_tools::agent_tools::SubagentToolExecutor;

/// The four request shapes zo speaks.
#[derive(Debug, Clone, Copy)]
enum Wire {
    Anthropic,
    ChatCompletions,
    Responses,
    GeminiCodeAssist,
}

impl Wire {
    /// The provider family whose shipped model the request names, read from
    /// the catalog rather than copied.
    fn model(self) -> &'static str {
        let family = match self {
            Self::Anthropic => api::ProviderKind::Anthropic,
            Self::ChatCompletions | Self::Responses => api::ProviderKind::OpenAi,
            Self::GeminiCodeAssist => api::ProviderKind::Google,
        };
        api::builtin_provider_catalog()
            .iter()
            .find(|entry| entry.provider == family)
            .unwrap_or_else(|| panic!("the shipped catalog has no {family:?} model"))
            .canonical_model_id
    }

    /// The provider's real client, on the fake server at `base_url`, with a
    /// synthetic credential. Gemini Code Assist reads its endpoint from the
    /// environment (`CODE_ASSIST_ENDPOINT`), set by the caller.
    fn client(self, base_url: &str) -> api::ProviderClient {
        match self {
            Self::Anthropic => api::ProviderClient::Anthropic(
                api::AnthropicClient::from_auth(api::AuthSource::ApiKey(SYNTHETIC_KEY.to_string()))
                    .with_base_url(base_url),
            ),
            Self::ChatCompletions => api::ProviderClient::OpenAi(
                api::OpenAiCompatClient::new(SYNTHETIC_KEY, api::OpenAiCompatConfig::openai())
                    .with_base_url(base_url),
            ),
            Self::Responses => api::ProviderClient::ChatGpt(
                api::ChatGptBackendClient::new(SYNTHETIC_KEY, None).with_base_url(base_url),
            ),
            Self::GeminiCodeAssist => {
                api::ProviderClient::GeminiCodeAssist(api::GeminiCodeAssistClient::new(SYNTHETIC_KEY))
            }
        }
    }

    /// One model turn in this provider's streaming shape.
    fn reply(self, turn: &Turn, at: usize) -> String {
        match self {
            Self::Anthropic => anthropic_reply(turn, at, self.model()),
            Self::ChatCompletions => chat_completions_reply(turn, at, self.model()),
            Self::Responses => responses_reply(turn, at, self.model()),
            Self::GeminiCodeAssist => gemini_reply(turn),
        }
    }
}

const SYNTHETIC_KEY: &str = "synthetic-key";
const FIXTURE: &str = "fixture.txt";
const DONE: &str = "fixture fixed";
/// What the command prints once the file is there.
const RAN: &str = "command-ran";

/// One model turn the fake provider answers with.
enum Turn {
    Call { name: &'static str, input: Value },
    Say(&'static str),
}

/// The names a model calls the tools by.
#[derive(Debug, Clone, Copy)]
enum Names {
    /// What zo shows every model: `Read` with `file_path`.
    General,
    /// What the handlers are written under: `read_file` with `path`.
    Handlers,
}

/// The task as the model works it: read the file, fix it, run a command.
fn task(fixture: &str, names: Names) -> Vec<Turn> {
    let (name, file_field): (fn(CoreTool) -> &'static str, &str) = match names {
        Names::General => (CoreTool::wire, field_to_wire(CoreTool::Read.handler(), "path")),
        Names::Handlers => (CoreTool::handler, "path"),
    };
    let mut read = json!({});
    read[file_field] = Value::from(fixture);
    let mut edit = json!({ "old_string": "bug", "new_string": "fix" });
    edit[file_field] = Value::from(fixture);
    vec![
        Turn::Call { name: name(CoreTool::Read), input: read },
        Turn::Call { name: name(CoreTool::Edit), input: edit },
        Turn::Call {
            name: name(CoreTool::Bash),
            input: json!({ "command": format!("test -f '{fixture}' && printf {RAN}") }),
        },
        Turn::Say(DONE),
    ]
}

fn sse_events(events: &[(&str, Value)]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for (event, data) in events {
        let _ = write!(out, "event: {event}\ndata: {data}\n\n");
    }
    out
}

fn data_frames(frames: &[Value]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for frame in frames {
        let _ = write!(out, "data: {frame}\n\n");
    }
    out
}

fn anthropic_reply(turn: &Turn, at: usize, model: &str) -> String {
    let usage = json!({
        "input_tokens": 1,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
        "output_tokens": 1
    });
    let (block, delta, stop) = match turn {
        Turn::Call { name, input } => (
            json!({ "type": "tool_use", "id": format!("toolu_{at}"), "name": name, "input": {} }),
            json!({ "type": "input_json_delta", "partial_json": input.to_string() }),
            "tool_use",
        ),
        Turn::Say(text) => (
            json!({ "type": "text", "text": "" }),
            json!({ "type": "text_delta", "text": text }),
            "end_turn",
        ),
    };
    sse_events(&[
        ("message_start", json!({ "type": "message_start", "message": {
            "id": format!("msg_{at}"), "type": "message", "role": "assistant", "content": [],
            "model": model, "stop_reason": null, "stop_sequence": null, "usage": usage
        }})),
        ("content_block_start", json!({ "type": "content_block_start", "index": 0, "content_block": block })),
        ("content_block_delta", json!({ "type": "content_block_delta", "index": 0, "delta": delta })),
        ("content_block_stop", json!({ "type": "content_block_stop", "index": 0 })),
        ("message_delta", json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop, "stop_sequence": null },
            "usage": usage
        })),
        ("message_stop", json!({ "type": "message_stop" })),
    ])
}

fn chat_completions_reply(turn: &Turn, at: usize, model: &str) -> String {
    let (delta, finish) = match turn {
        Turn::Call { name, input } => (
            json!({ "tool_calls": [{
                "index": 0, "id": format!("call_{at}"), "type": "function",
                "function": { "name": name, "arguments": input.to_string() }
            }]}),
            "tool_calls",
        ),
        Turn::Say(text) => (json!({ "content": text }), "stop"),
    };
    let id = format!("chatcmpl_{at}");
    data_frames(&[
        json!({ "id": id, "model": model, "choices": [{ "index": 0, "delta": delta }] }),
        json!({ "id": id, "choices": [{ "index": 0, "delta": {}, "finish_reason": finish }] }),
    ]) + "data: [DONE]\n\n"
}

fn responses_reply(turn: &Turn, at: usize, model: &str) -> String {
    let (opened, step, item) = match turn {
        Turn::Call { name, input } => {
            let (id, call_id) = (format!("fc_{at}"), format!("call_{at}"));
            (
                json!({ "type": "function_call", "id": id, "call_id": call_id, "name": name, "arguments": "" }),
                ("response.function_call_arguments.delta", json!({
                    "type": "response.function_call_arguments.delta",
                    "output_index": 0, "item_id": id, "delta": input.to_string()
                })),
                json!({
                    "type": "function_call", "id": id, "call_id": call_id, "name": name,
                    "arguments": input.to_string(), "status": "completed"
                }),
            )
        }
        Turn::Say(text) => {
            let id = format!("msg_{at}");
            (
                json!({ "type": "message", "id": id, "role": "assistant", "content": [] }),
                ("response.output_text.delta", json!({
                    "type": "response.output_text.delta",
                    "output_index": 0, "item_id": id, "content_index": 0, "delta": text
                })),
                json!({
                    "type": "message", "id": id, "role": "assistant", "status": "completed",
                    "content": [{ "type": "output_text", "text": text, "annotations": [] }]
                }),
            )
        }
    };
    let response = |status: &str, output: Value| json!({
        "id": format!("resp_{at}"), "object": "response", "model": model, "status": status,
        "output": output,
        "usage": {
            "input_tokens": 1, "output_tokens": 1, "total_tokens": 2,
            "input_tokens_details": { "cached_tokens": 0 },
            "output_tokens_details": { "reasoning_tokens": 0 }
        }
    });
    sse_events(&[
        ("response.created", json!({ "type": "response.created", "response": response("in_progress", json!([])) })),
        ("response.output_item.added", json!({ "type": "response.output_item.added", "output_index": 0, "item": opened })),
        step,
        ("response.output_item.done", json!({ "type": "response.output_item.done", "output_index": 0, "item": item })),
        ("response.completed", json!({ "type": "response.completed", "response": response("completed", json!([item])) })),
    ])
}

fn gemini_reply(turn: &Turn) -> String {
    let part = match turn {
        Turn::Call { name, input } => json!({ "functionCall": { "name": name, "args": input } }),
        Turn::Say(text) => json!({ "text": text }),
    };
    data_frames(&[json!({ "response": {
        "candidates": [{ "content": { "role": "model", "parts": [part] }, "finishReason": "STOP" }],
        "usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 1, "totalTokenCount": 2 }
    }})])
}

/// Gemini Code Assist's account setup answer: a project already chosen.
const CODE_ASSIST_SETUP: &str = r#"{"currentTier":{"id":"standard-tier"},"cloudaicompanionProject":"wire-names-fixture"}"#;

/// A provider on a local port that answers its N-th model request with the
/// task's N-th turn (the last one again past the end) and keeps every model
/// request's body. Each connection is served on a thread of its own.
fn fake_provider(wire: Wire, script: Vec<Turn>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a local port");
    let base_url = format!("http://{}", listener.local_addr().expect("its address"));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    let script = Arc::new(script);
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                continue;
            };
            let (seen, script) = (Arc::clone(&seen), Arc::clone(&script));
            std::thread::spawn(move || {
                let Some((head, body)) = read_request(&mut connection) else {
                    return;
                };
                let (content_type, answer) = if head.contains(":loadcodeassist") {
                    ("application/json", CODE_ASSIST_SETUP.to_string())
                } else {
                    let mut seen = seen.lock().expect("bodies");
                    let at = seen.len();
                    seen.push(body);
                    let turn = &script[at.min(script.len() - 1)];
                    ("text/event-stream", wire.reply(turn, at))
                };
                let _ = write!(
                    connection,
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            });
        }
    });
    (base_url, bodies)
}

/// A request's lower-cased head and its body, read to the end its length says.
fn read_request(connection: &mut std::net::TcpStream) -> Option<(String, String)> {
    let mut seen = Vec::new();
    let mut chunk = [0_u8; 8192];
    let head_end = loop {
        if let Some(at) = seen.windows(4).position(|four| four == b"\r\n\r\n") {
            break at + 4;
        }
        let filled = connection.read(&mut chunk).ok().filter(|filled| *filled > 0)?;
        seen.extend_from_slice(&chunk[..filled]);
    };
    let head = String::from_utf8_lossy(&seen[..head_end]).to_ascii_lowercase();
    let length = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|length| length.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while seen.len() < head_end + length {
        let filled = connection.read(&mut chunk).ok().filter(|filled| *filled > 0)?;
        seen.extend_from_slice(&chunk[..filled]);
    }
    Some((head, String::from_utf8_lossy(&seen[head_end..]).into_owned()))
}

/// The task, done through `wire` under `names`, in a fresh workspace.
fn finish_the_task(wire: Wire, names: Names) {
    let workspace = tempfile::tempdir().expect("a workspace");
    let fixture = workspace.path().join(FIXTURE);
    std::fs::write(&fixture, "bug\n").expect("the fixture");
    let fixture_text = fixture.display().to_string();
    let (base_url, bodies) = fake_provider(wire, task(&fixture_text, names));
    // Gemini Code Assist finds its endpoint in the environment; the guard
    // holds the crate's environment lock for the whole task either way.
    let _environment = crate::tests::EnvGuard::set("CODE_ASSIST_ENDPOINT", &base_url);
    let driver = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let allowed: BTreeSet<String> = [CoreTool::Read, CoreTool::Edit, CoreTool::Bash]
        .into_iter()
        .map(|tool| tool.handler().to_string())
        .collect();
    let agent = ProviderRuntimeClient::for_tests(
        wire.client(&base_url),
        wire.model(),
        allowed.clone(),
        driver.handle().clone(),
    );
    let mut tools = SubagentToolExecutor::new(allowed);
    tools.context = crate::ToolContext::new()
        .with_workspace_root(workspace.path())
        .with_cwd(workspace.path());
    let mut conversation = ConversationRuntime::new(
        Session::new(),
        agent,
        tools,
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["You fix files.".to_string()],
    );

    let summary = conversation
        .run_turn("Fix the bug in the fixture, then check it is there.", None)
        .unwrap_or_else(|error| panic!("{wire:?} {names:?}: the turn failed: {error}"));

    let what = format!("{wire:?} {names:?}");
    assert_eq!(std::fs::read_to_string(&fixture).expect("read back"), "fix\n", "{what}");
    let results: Vec<(String, bool)> = summary
        .tool_results
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { output, is_error, .. } => Some((output.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 3, "{what}: {results:?}");
    assert!(results.iter().all(|(_, is_error)| !is_error), "{what}: {results:?}");
    assert!(results[2].0.contains(RAN), "{what}: the command did not run: {results:?}");
    let bodies = bodies.lock().expect("bodies");
    assert_eq!(bodies.len(), 4, "{what}: one request per turn");
    // A name is a JSON string of its own wherever a provider carries one.
    let quoted = |name: &str| format!("\"{name}\"");
    for body in bodies.iter() {
        for tool in [CoreTool::Read, CoreTool::Edit, CoreTool::Bash] {
            assert!(body.contains(&quoted(tool.wire())), "{what}: {} is not shown", tool.wire());
            assert!(
                !body.contains(&quoted(tool.handler())),
                "{what}: the handler's {} reached the model: {body}",
                tool.handler()
            );
        }
        assert_eq!(
            body.matches("path").count(),
            body.matches(field_to_wire(CoreTool::Read.handler(), "path")).count(),
            "{what}: a handler's `path` reached the model: {body}"
        );
    }
}

#[test]
fn anthropic_finishes_a_task_under_the_general_and_the_handlers_names() {
    for names in [Names::General, Names::Handlers] {
        finish_the_task(Wire::Anthropic, names);
    }
}

#[test]
fn chat_completions_finishes_a_task_under_the_general_and_the_handlers_names() {
    for names in [Names::General, Names::Handlers] {
        finish_the_task(Wire::ChatCompletions, names);
    }
}

#[test]
fn responses_finishes_a_task_under_the_general_and_the_handlers_names() {
    for names in [Names::General, Names::Handlers] {
        finish_the_task(Wire::Responses, names);
    }
}

#[test]
fn gemini_code_assist_finishes_a_task_under_the_general_and_the_handlers_names() {
    for names in [Names::General, Names::Handlers] {
        finish_the_task(Wire::GeminiCodeAssist, names);
    }
}
