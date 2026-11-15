//! Two fresh processes talk to a local server; only the temporary home is shared.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::time::{Duration, Instant};

use api::{InputContentBlock, InputMessage, MessageRequest, OpenAiCompatClient, OpenAiCompatConfig};
use runtime::{ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, PermissionMode,
    PermissionPolicy, RuntimeError, Session, StaticToolExecutor, WireTarget};
use serde_json::{Value, json};

const MODEL: &str = "acme-scene-1";
const CEILING: usize = 28_000;

#[derive(Debug, Clone)]
struct Seen {
    session: usize,
    status: u16,
    bytes: usize,
    pictures: usize,
    prefix: String,
}

struct Endpoint {
    url: String,
    phase: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Vec<Seen>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn read_body(stream: &mut TcpStream) -> Vec<u8> {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = stream.read(&mut chunk).unwrap();
        assert_ne!(count, 0, "a complete HTTP request");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end]);
            let length = header.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
            }).unwrap();
            if bytes.len() >= end + 4 + length { return bytes[end + 4..end + 4 + length].to_vec(); }
        }
    }
}

impl Endpoint {
    fn new(mode: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let phase = Arc::new(AtomicUsize::new(1));
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (worker_phase, worker_stop, worker_seen) = (phase.clone(), stop.clone(), seen.clone());
        let thread = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(90);
            while !worker_stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("local endpoint: {error}"),
                };
                let body = read_body(&mut stream);
                let value: Value = serde_json::from_slice(&body).unwrap();
                let text = String::from_utf8_lossy(&body);
                let pictures = text.matches("data:image/png;base64,").count();
                let reasoning = value["messages"].as_array().unwrap().iter().any(|message| message.get("reasoning_content").is_some());
                let (status, phrase) = match mode {
                    "reasoning" if reasoning => (400, "Bad Request"),
                    "required" if !reasoning => (400, "Bad Request"),
                    "body" if body.len() > CEILING => (413, "Payload Too Large"),
                    "rate" => (429, "Too Many Requests"),
                    "server" => (503, "Service Unavailable"),
                    _ => (200, "OK"),
                };
                worker_seen.lock().unwrap().push(Seen {
                    session: worker_phase.load(Ordering::Relaxed), status, bytes: body.len(), pictures,
                    prefix: {
                        use sha2::{Digest as _, Sha256};
                        format!("{:x}", Sha256::digest(&body))
                    },
                });
                let response = if mode == "anthropic" {
                    json!({"id":"scene","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":1}})
                } else if status == 200 {
                    json!({"id":"scene","model":MODEL,"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":1}})
                } else {
                    json!({"error":{"message":if mode == "required" {"reasoning_content is required"} else if status == 400 {"extra inputs are not permitted: reasoning_content"} else {"request refused"},"type":"invalid_request_error"}})
                }.to_string();
                write!(stream, "HTTP/1.1 {status} {phrase}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}", response.len()).unwrap();
            }
        });
        Self { url, phase, stop, seen, thread: Some(thread) }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() { thread.join().unwrap(); }
    }
}

fn child(endpoint: &Endpoint, home: &Path, mode: &str) {
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "capability_scene_child", "--nocapture"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("ZO_CONFIG_HOME", home)
        .env("ZO_HOME", home)
        .env("ZO_DISABLE_KEYCHAIN", "1")
        .env("ZO_DISABLE_MODEL_DISCOVERY", "1")
        .env("ZEROCODE_SECOND_BRAIN", "off")
        .env("ZO_EXPERIMENTAL_PROVIDER_ADAPTERS", "1")
        .env("ZO_CAPABILITY_SCENE", mode)
        .env("ZO_CAPABILITY_ENDPOINT", &endpoint.url)
        .env(api::CUSTOM_PROVIDERS_ENV, if mode.starts_with("prefix-") { "[]".to_string() } else {
            json!([{"name":"acme","base_url":endpoint.url,"models":[MODEL],"requires_auth":false,"supports_vision":true}]).to_string()
        })
        .current_dir(&workspace)
        .stdin(Stdio::null()).output().unwrap();
    assert!(output.status.success(), "child failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

fn request(messages: Vec<InputMessage>) -> MessageRequest {
    MessageRequest {
        model: MODEL.to_string(), max_tokens: 64, messages, system: None, tools: None,
        tool_choice: None, stream: false, thinking: None, output_config: None,
        effort: None, effort_band_ceiling: None,
    }
}

fn picture_session(mode: &str) -> Session {
    let mut session = Session::new();
    if mode == "tool" {
        session.push_user_text("stored tool pictures").unwrap();
        for index in 0..8 {
            let id = format!("picture-{index}");
            session.push_message(runtime::ConversationMessage::assistant(vec![runtime::ContentBlock::ToolUse {
                id:id.clone(), name:"read_image".to_string(), input:"{}".to_string(),
            }])).unwrap();
            session.push_message(runtime::ConversationMessage::tool_result_with_images(id, "read_image", "picture", false,
                vec![("image/png".to_string(), "A".repeat(4_000))])).unwrap();
        }
    } else {
        let (picture_count, bytes) = match mode { "body" | "warm" => (8, 4_000), "pair" => (2, 16_000), _ => (3, 4_000) };
        session.push_message(runtime::ConversationMessage::user_with_images("pictures", (0..picture_count)
            .map(|_| ("image/png".to_string(), "A".repeat(bytes))).collect())).unwrap();
    }
    session
}

struct LocalApi {
    client: OpenAiCompatClient,
    rt: tokio::runtime::Runtime,
}

impl ApiClient for LocalApi {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let target = WireTarget::for_model(MODEL).with_picture_cap(request.picture_cap);
        let wire = self.rt.block_on(self.client.send_message(&self::request(
            runtime::convert_messages_for(&request.messages, target),
        ))).map_err(|error| RuntimeError::with_provider_error_class(error.to_string(), error.provider_error_class()))?;
        assert_eq!(wire.model, MODEL);
        Ok(vec![AssistantEvent::TextDelta("done".to_string()), AssistantEvent::MessageStop])
    }
}

#[test]
#[ignore = "launched by the parent with a temporary home and local endpoint"]
fn capability_scene_child() {
    let endpoint = std::env::var("ZO_CAPABILITY_ENDPOINT").unwrap();
    let mode = std::env::var("ZO_CAPABILITY_SCENE").unwrap();
    let mut config = OpenAiCompatConfig::from_user("acme", &endpoint, None, false);
    config.supports_vision = true;
    let client = OpenAiCompatClient::new("fixture", config).with_retry_policy(0, Duration::ZERO, Duration::ZERO);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    if let Some(family) = mode.strip_prefix("prefix-") {
        let (model, config) = match family {
            "claude" => ("claude-opus-5-5", None),
            "gpt" => ("gpt-6.1-sol", Some(OpenAiCompatConfig::openai())),
            "gemini" => ("gemini-3.6-flash", Some(OpenAiCompatConfig::google())),
            _ => panic!("unknown fixture family"),
        };
        let cwd = std::env::current_dir().unwrap();
        let system = runtime::load_system_prompt_for_main_with_mode(&cwd, "2026-01-01", std::env::consts::OS,
            "unknown", Some(model), runtime::PromptMode::Autonomous).unwrap();
        let mut wire = request(vec![InputMessage::user_text("inspect the fixture")]);
        wire.model = model.to_string();
        wire.system = Some(system.into_iter().flat_map(api::system_from_string).collect());
        if let Some(config) = config {
            let client = OpenAiCompatClient::new("fixture", config).with_base_url(endpoint);
            rt.block_on(client.send_message(&wire)).unwrap();
        } else {
            let client = api::AnthropicClient::new("fixture").with_base_url(endpoint.trim_end_matches("/v1"));
            rt.block_on(client.send_message(&wire)).unwrap();
        }
    } else if mode == "reasoning" || mode == "required" {
        let mut content = vec![InputContentBlock::Text { text:"saved answer".to_string(), cache_control:None }];
        if mode == "reasoning" { content.insert(0, InputContentBlock::Thinking { thinking:"saved reasoning".to_string(), signature:String::new() }); }
        let history = vec![InputMessage::user_text("continue"), InputMessage {
            role:"assistant".to_string(), content, thought_signature:None, reasoning_replay:None,
        }];
        rt.block_on(client.send_message(&request(history))).unwrap();
    } else if matches!(mode.as_str(), "body" | "below" | "pair" | "warm" | "tool") {
        if mode == "warm" {
            rt.block_on(client.send_message(&request(vec![InputMessage::user_text("small request")]))).unwrap();
        }
        let session = picture_session(&mode);
        let original = session.messages.clone();
        let mut runtime = ConversationRuntime::new(session, LocalApi { client, rt }, StaticToolExecutor::new(), PermissionPolicy::new(PermissionMode::DangerFullAccess), vec!["system".to_string()]);
        runtime.set_context_model(MODEL);
        runtime.set_workspace_cwd(std::env::current_dir().unwrap());
        runtime.run_turn("inspect", None).unwrap();
        assert_eq!(&runtime.session().messages[..original.len()], original.as_slice(), "stored images stay byte-identical");
    } else {
        assert!(rt.block_on(client.send_message(&request(vec![InputMessage::user_text("hello")]))).is_err());
    }
}

#[test]
fn a_second_process_sends_reasoning_without_another_refusal() {
    for mode in ["reasoning", "required"] {
        let home = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::new(mode);
        child(&endpoint, home.path(), mode);
        endpoint.phase.store(2, Ordering::Relaxed);
        child(&endpoint, home.path(), mode);
        let seen = endpoint.seen.lock().unwrap();
        assert_eq!(seen.iter().filter(|row| row.session == 1 && row.status == 400).count(), 1);
        assert_eq!(seen.iter().filter(|row| row.session == 2 && row.status == 400).count(), 0, "second session repeats no reasoning refusal ({mode}): {seen:?}");
    }
}

#[test]
fn a_second_process_budgets_before_sending_and_preserves_the_transcript() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::new("body");
    child(&endpoint, home.path(), "body");
    endpoint.phase.store(2, Ordering::Relaxed);
    child(&endpoint, home.path(), "body");
    let seen = endpoint.seen.lock().unwrap();
    assert_eq!(seen.iter().filter(|row| row.session == 1 && row.status == 413).count(), 1);
    assert_eq!(seen.iter().filter(|row| row.session == 2 && row.status == 413).count(), 0, "second session repeats no body refusal: {seen:?}");
    let first_success = seen.iter().find(|row| row.status == 200).unwrap();
    assert!(first_success.pictures > 2, "recovery uses the byte steps: {seen:?}");
    assert!(seen.iter().filter(|row| row.status == 200).all(|row| row.bytes <= CEILING));
    let files = std::fs::read_dir(home.path().join("cache/endpoint-capabilities")).unwrap();
    let records: Vec<Value> = files.filter_map(Result::ok).filter(|entry| entry.path().extension().is_some_and(|extension| extension == "json"))
        .map(|entry| serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap()).collect();
    assert_eq!(records.len(), 1);
    let bounds = &records[0]["body"]["value"];
    assert_eq!(bounds["rejected_bytes"], seen.iter().find(|row| row.status == 413).unwrap().bytes);
    assert_eq!(bounds["largest_accepted_bytes"], seen.iter().filter(|row| row.status == 200).map(|row| row.bytes).max().unwrap());
}

#[test]
fn rate_limits_and_server_errors_leave_no_learned_record() {
    for mode in ["rate", "server"] {
        let home = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::new(mode);
        child(&endpoint, home.path(), mode);
        let cache = home.path().join("cache/endpoint-capabilities");
        assert!(!cache.exists(), "{mode} is not capability evidence");
    }
}

#[test]
fn a_body_below_the_limit_keeps_its_pictures() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::new("body");
    child(&endpoint, home.path(), "below");
    endpoint.phase.store(2, Ordering::Relaxed);
    child(&endpoint, home.path(), "below");
    let seen = endpoint.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|row| row.status == 200 && row.pictures == 3), "{seen:?}");
}

/// The real system-prompt builder and each first-party encoder, with a local
/// HTTP server. The worker gate compares these hashes with the old tree.
#[test]
fn first_party_request_prefixes_are_recorded_for_comparison() {
    let temporary = tempfile::tempdir().unwrap();
    let home = std::env::var_os("ZO_PREFIX_SCENE_HOME").map(std::path::PathBuf::from)
        .unwrap_or_else(|| temporary.path().to_path_buf());
    std::fs::create_dir_all(&home).unwrap();
    let mut hashes = serde_json::Map::new();
    for (family, server_mode) in [("claude", "anthropic"), ("gpt", "prefix"), ("gemini", "prefix")] {
        let endpoint = Endpoint::new(server_mode);
        child(&endpoint, &home, &format!("prefix-{family}"));
        let seen = endpoint.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        hashes.insert(family.to_string(), json!(seen[0].prefix));
    }
    if let Some(path) = std::env::var_os("ZO_PREFIX_SNAPSHOT") {
        std::fs::write(path, serde_json::to_vec_pretty(&hashes).unwrap()).unwrap();
    }
    eprintln!("first-party request prefix hashes: {}", Value::Object(hashes));
}

#[test]
fn the_exact_preflight_recovers_when_the_latest_pair_exceeds_the_learned_limit() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::new("body");
    child(&endpoint, home.path(), "pair");
    endpoint.phase.store(2, Ordering::Relaxed);
    child(&endpoint, home.path(), "pair");
    let seen = endpoint.seen.lock().unwrap();
    assert_eq!(seen.iter().filter(|row| row.session == 1 && row.status == 413).count(), 1);
    assert_eq!(seen.iter().filter(|row| row.session == 2 && row.status == 413).count(), 0, "the local preflight applies the next step before a second remote refusal: {seen:?}");
    assert!(seen.iter().filter(|row| row.status == 200).all(|row| row.pictures == 1));
}

#[test]
fn a_small_prior_success_does_not_prevent_picture_recovery() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::new("body");
    child(&endpoint, home.path(), "warm");
    endpoint.phase.store(2, Ordering::Relaxed);
    child(&endpoint, home.path(), "warm");
    let seen = endpoint.seen.lock().unwrap();
    assert_eq!(seen.iter().filter(|row| row.session == 1 && row.status == 413).count(), 1);
    assert_eq!(seen.iter().filter(|row| row.session == 2 && row.status == 413).count(), 0);
    assert!(seen.iter().any(|row| row.session == 2 && row.status == 200 && row.pictures >= 2));
}

#[test]
fn stored_tool_pictures_survive_replay_without_a_second_refusal() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::new("body");
    child(&endpoint, home.path(), "tool");
    endpoint.phase.store(2, Ordering::Relaxed);
    child(&endpoint, home.path(), "tool");
    let seen = endpoint.seen.lock().unwrap();
    assert_eq!(seen.iter().filter(|row| row.session == 1 && row.status == 413).count(), 1);
    assert_eq!(seen.iter().filter(|row| row.session == 2 && row.status == 413).count(), 0, "{seen:?}");
    assert!(seen.iter().any(|row| row.session == 2 && row.status == 200 && row.pictures >= 2));
}
