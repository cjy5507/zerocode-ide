use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::Arc;
use std::sync::{Mutex as StdMutex, OnceLock};

use api::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, InputMessage, MessageDeltaEvent, MessageRequest, OpenAiCompatClient,
    OpenAiCompatConfig, OutputContentBlock, ProviderClient, StreamEvent, ToolChoice,
    ToolDefinition, EXPERIMENTAL_PROVIDERS_ENV, NON_CLAUDE_ADAPTERS_ENV,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[tokio::test]
async fn custom_openai_base_url_skips_official_prompt_cache_controls() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let body = concat!(
        "{",
        "\"id\":\"chatcmpl_custom_openai\",",
        "\"model\":\"gpt-5.5\",",
        "\"choices\":[{",
        "\"message\":{\"role\":\"assistant\",\"content\":\"ok\",\"tool_calls\":[]},",
        "\"finish_reason\":\"stop\"",
        "}],",
        "\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":1,",
        "\"prompt_tokens_details\":{\"cached_tokens\":8}}",
        "}"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "application/json", body)],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("openai-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let response = client
        .send_message(&MessageRequest {
            model: "gpt-5.5".to_string(),
            ..sample_request(false)
        })
        .await
        .expect("request should succeed");

    assert_eq!(response.usage.input_tokens, 10);
    assert_eq!(response.usage.cache_read_input_tokens, 0);
    let captured = state.lock().await;
    let request = captured.first().expect("server should capture request");
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert!(body.get("prompt_cache_key").is_none());
    assert!(body.get("prompt_cache_retention").is_none());
}

#[tokio::test]
async fn send_message_uses_openai_compatible_endpoint_and_auth() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let body = concat!(
        "{",
        "\"id\":\"chatcmpl_test\",",
        "\"model\":\"grok-3\",",
        "\"choices\":[{",
        "\"message\":{\"role\":\"assistant\",\"content\":\"Hello from Grok\",\"tool_calls\":[]},",
        "\"finish_reason\":\"stop\"",
        "}],",
        "\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":5}",
        "}"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "application/json", body)],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("xai-test-key", OpenAiCompatConfig::xai())
        .with_base_url(server.base_url());
    let response = client
        .send_message(&sample_request(false))
        .await
        .expect("request should succeed");

    assert_eq!(response.model, "grok-3");
    assert_eq!(response.total_tokens(), 16);
    assert_eq!(
        response.content,
        vec![OutputContentBlock::Text {
            text: "Hello from Grok".to_string(),
        }]
    );

    let captured = state.lock().await;
    let request = captured.first().expect("server should capture request");
    assert_eq!(request.path, "/chat/completions");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer xai-test-key")
    );
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["model"], json!("grok-3"));
    assert_eq!(body["messages"][0]["role"], json!("system"));
    assert_eq!(body["tools"][0]["type"], json!("function"));
}

#[tokio::test]
async fn send_message_accepts_full_chat_completions_endpoint_override() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let body = concat!(
        "{",
        "\"id\":\"chatcmpl_full_endpoint\",",
        "\"model\":\"grok-3\",",
        "\"choices\":[{",
        "\"message\":{\"role\":\"assistant\",\"content\":\"Endpoint override works\",\"tool_calls\":[]},",
        "\"finish_reason\":\"stop\"",
        "}],",
        "\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}",
        "}"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "application/json", body)],
    )
    .await
    else {
        return;
    };

    let endpoint_url = format!("{}/chat/completions", server.base_url());
    let client = OpenAiCompatClient::new("xai-test-key", OpenAiCompatConfig::xai())
        .with_base_url(endpoint_url);
    let response = client
        .send_message(&sample_request(false))
        .await
        .expect("request should succeed");

    assert_eq!(response.total_tokens(), 10);

    let captured = state.lock().await;
    let request = captured.first().expect("server should capture request");
    assert_eq!(request.path, "/chat/completions");
}

#[tokio::test]
async fn stream_message_normalizes_text_and_multiple_tool_calls() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"id\":\"chatcmpl_stream\",\"model\":\"grok-3\",\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl_stream\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"weather\",\"arguments\":\"{\\\"city\\\":\\\"Paris\\\"}\"}},{\"index\":1,\"id\":\"call_2\",\"function\":{\"name\":\"clock\",\"arguments\":\"{\\\"zone\\\":\\\"UTC\\\"}\"}}]}}]}\n\n",
        "data: {\"id\":\"chatcmpl_stream\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response_with_headers(
            "200 OK",
            "text/event-stream",
            sse,
            &[("x-request-id", "req_grok_stream")],
        )],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("xai-test-key", OpenAiCompatConfig::xai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(false))
        .await
        .expect("stream should start");

    assert_eq!(stream.request_id(), Some("req_grok_stream"));

    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    assert!(matches!(events[0], StreamEvent::MessageStart(_)));
    assert!(matches!(
        events[1],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            content_block: OutputContentBlock::Text { .. },
            ..
        })
    ));
    assert!(matches!(
        events[2],
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            delta: ContentBlockDelta::TextDelta { .. },
            ..
        })
    ));
    assert!(matches!(
        events[3],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 1,
            content_block: OutputContentBlock::ToolUse { .. },
        })
    ));
    assert!(matches!(
        events[4],
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 1,
            delta: ContentBlockDelta::InputJsonDelta { .. },
        })
    ));
    assert!(matches!(
        events[5],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 2,
            content_block: OutputContentBlock::ToolUse { .. },
        })
    ));
    assert!(matches!(
        events[6],
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 2,
            delta: ContentBlockDelta::InputJsonDelta { .. },
        })
    ));
    assert!(matches!(
        events[7],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 1 })
    ));
    assert!(matches!(
        events[8],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 2 })
    ));
    assert!(matches!(
        events[9],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
    ));
    assert!(matches!(events[10], StreamEvent::MessageDelta(_)));
    assert!(matches!(events[11], StreamEvent::MessageStop(_)));

    let captured = state.lock().await;
    let request = captured.first().expect("captured request");
    assert_eq!(request.path, "/chat/completions");
    assert!(request.body.contains("\"stream\":true"));
}

#[tokio::test]
async fn openai_streaming_requests_opt_into_usage_chunks() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"id\":\"chatcmpl_openai_stream\",\"model\":\"gpt-5\",\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl_openai_stream\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"chatcmpl_openai_stream\",\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n\n",
        "data: [DONE]\n\n"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response_with_headers(
            "200 OK",
            "text/event-stream",
            sse,
            &[("x-request-id", "req_openai_stream")],
        )],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("openai-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(false))
        .await
        .expect("stream should start");

    assert_eq!(stream.request_id(), Some("req_openai_stream"));

    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    assert!(matches!(events[0], StreamEvent::MessageStart(_)));
    assert!(matches!(
        events[1],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            content_block: OutputContentBlock::Text { .. },
            ..
        })
    ));
    assert!(matches!(
        events[2],
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            delta: ContentBlockDelta::TextDelta { .. },
            ..
        })
    ));
    assert!(matches!(
        events[3],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
    ));
    assert!(matches!(
        events[4],
        StreamEvent::MessageDelta(MessageDeltaEvent { .. })
    ));
    assert!(matches!(events[5], StreamEvent::MessageStop(_)));

    match &events[4] {
        StreamEvent::MessageDelta(MessageDeltaEvent { usage, .. }) => {
            assert_eq!(usage.input_tokens, 9);
            assert_eq!(usage.output_tokens, 4);
        }
        other => panic!("expected message delta, got {other:?}"),
    }

    let captured = state.lock().await;
    let request = captured.first().expect("captured request");
    assert_eq!(request.path, "/chat/completions");
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
}

#[tokio::test]
async fn user_endpoint_streaming_uses_optional_auth_and_usage_chunks() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"id\":\"chatcmpl_local_stream\",\"model\":\"local-model\",\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl_local_stream\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"chatcmpl_local_stream\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":6}}\n\n",
        "data: [DONE]\n\n"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };

    let config = OpenAiCompatConfig::from_user("LM Studio", &server.base_url(), None, true);
    let client =
        OpenAiCompatClient::from_env_optional_auth(config).expect("keyless local endpoint");
    let mut request = sample_request(true);
    request.model = "local-model".to_string();
    let mut stream = client
        .stream_message(&request)
        .await
        .expect("stream should start");

    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    let delta = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::MessageDelta(delta) => Some(delta),
            _ => None,
        })
        .expect("message delta");
    assert_eq!(delta.usage.input_tokens, 12);
    assert_eq!(delta.usage.output_tokens, 6);

    let captured = state.lock().await;
    let request = captured.first().expect("captured request");
    assert_eq!(request.path, "/chat/completions");
    assert!(!request.headers.contains_key("authorization"));
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["model"], json!("local-model"));
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
}

#[tokio::test]
async fn streaming_without_usage_chunk_falls_back_to_zero_usage() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"id\":\"chatcmpl_no_usage\",\"model\":\"local-model\",\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl_no_usage\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };

    let config = OpenAiCompatConfig::from_user("vLLM", &server.base_url(), None, true);
    let client =
        OpenAiCompatClient::from_env_optional_auth(config).expect("keyless local endpoint");
    let mut request = sample_request(true);
    request.model = "local-model".to_string();
    let mut stream = client
        .stream_message(&request)
        .await
        .expect("stream should start");

    let mut usage = None;
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        if let StreamEvent::MessageDelta(delta) = event {
            usage = Some(delta.usage);
        }
    }

    let usage = usage.expect("message delta usage");
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);

    let captured = state.lock().await;
    let request = captured.first().expect("captured request");
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
}

// Single-threaded test; the env lock across `.await` only serialises env access.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn provider_client_dispatches_xai_requests_from_env() {
    let _lock = env_lock();
    let _experimental_gate = ScopedEnvVar::set(EXPERIMENTAL_PROVIDERS_ENV, "1");
    let _provider_gate = ScopedEnvVar::set(NON_CLAUDE_ADAPTERS_ENV, "1");
    let _api_key = ScopedEnvVar::set("XAI_API_KEY", "xai-test-key");

    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response(
            "200 OK",
            "application/json",
            "{\"id\":\"chatcmpl_provider\",\"model\":\"grok-3\",\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"Through provider client\",\"tool_calls\":[]},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}",
        )],
    )
    .await else {
        return;
    };
    let _base_url = ScopedEnvVar::set("XAI_BASE_URL", server.base_url());

    let client =
        ProviderClient::from_model("grok").expect("xAI provider client should be constructed");
    assert!(matches!(client, ProviderClient::Xai(_)));

    let response = client
        .send_message(&sample_request(false))
        .await
        .expect("provider-dispatched request should succeed");

    assert_eq!(response.total_tokens(), 13);

    let captured = state.lock().await;
    let request = captured.first().expect("captured request");
    assert_eq!(request.path, "/chat/completions");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer xai-test-key")
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedRequest {
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

struct TestServer {
    base_url: String,
    join_handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    fn base_url(&self) -> String {
        self.base_url.clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn spawn_server(
    state: Arc<Mutex<Vec<CapturedRequest>>>,
    responses: Vec<String>,
) -> Option<TestServer> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping integration test: listener bind is not permitted in this environment"
            );
            return None;
        }
        Err(error) => panic!("listener should bind: {error}"),
    };
    let address = listener.local_addr().expect("listener addr");
    let join_handle = tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = Vec::new();
            let mut header_end = None;
            loop {
                let mut chunk = [0_u8; 1024];
                let read = socket.read(&mut chunk).await.expect("read request");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(position) = find_header_end(&buffer) {
                    header_end = Some(position);
                    break;
                }
            }

            let header_end = header_end.expect("headers should exist");
            let (header_bytes, remaining) = buffer.split_at(header_end);
            let header_text = String::from_utf8(header_bytes.to_vec()).expect("utf8 headers");
            let mut lines = header_text.split("\r\n");
            let request_line = lines.next().expect("request line");
            let path = request_line
                .split_whitespace()
                .nth(1)
                .expect("path")
                .to_string();
            let mut headers = HashMap::new();
            let mut content_length = 0_usize;
            for line in lines {
                if line.is_empty() {
                    continue;
                }
                let (name, value) = line.split_once(':').expect("header");
                let value = value.trim().to_string();
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse().expect("content length");
                }
                headers.insert(name.to_ascii_lowercase(), value);
            }

            let mut body = remaining[4..].to_vec();
            while body.len() < content_length {
                let mut chunk = vec![0_u8; content_length - body.len()];
                let read = socket.read(&mut chunk).await.expect("read body");
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read]);
            }

            state.lock().await.push(CapturedRequest {
                path,
                headers,
                body: String::from_utf8(body).expect("utf8 body"),
            });

            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        }
    });

    Some(TestServer {
        base_url: format!("http://{address}"),
        join_handle,
    })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn http_response(status: &str, content_type: &str, body: &str) -> String {
    http_response_with_headers(status, content_type, body, &[])
}

fn http_response_with_headers(
    status: &str,
    content_type: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> String {
    let mut extra_headers = String::new();
    for (name, value) in headers {
        use std::fmt::Write as _;
        write!(&mut extra_headers, "{name}: {value}\r\n").expect("header write");
    }
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\n{extra_headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn sample_request(stream: bool) -> MessageRequest {
    MessageRequest {
        model: "grok-3".to_string(),
        max_tokens: 64,
        messages: vec![InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::Text {
                text: "Say hello".to_string(),
                cache_control: None,
            }],
            thought_signature: None,
            reasoning_replay: None,
        }],
        system: Some(api::system_from_string("Use tools when needed")),
        tools: Some(vec![ToolDefinition {
            name: "weather".to_string(),
            description: Some("Fetches weather".to_string()),
            input_schema: json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"]
            }),
        }]),
        tool_choice: Some(ToolChoice::Auto),
        stream,
        thinking: None,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    }
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}
