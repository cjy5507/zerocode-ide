//! t-37798 — the production request path against a body ceiling.
//!
//! A Computer Use session stores one screenshot per step and re-sends the whole
//! history every step. Compaction summarizes the old prefix and keeps a
//! token-budgeted tail whole — 40k tokens on a 1M window, which is two dozen
//! pictures of ~1.2–2 MB each — so the request reached the provider's byte
//! ceiling (32 MiB) with most of the context window free, and the turn ended on
//! `413 request_too_large`: the client's own preflight against the catalog's
//! `max_request_bytes`, or the provider's answer on the wire.
//!
//! These tests drive the real thing end to end: a `ConversationRuntime` turn on
//! the live async client (`build_message_request` → the Anthropic client's
//! preflight → HTTP) against a local endpoint, so the byte sizes are the ones a
//! provider would be sent. The pictures are synthetic: a real tiny PNG header
//! padded to size, as a real screenshot of that many bytes reads to the guard.

use std::sync::{Arc, Mutex};

use api::{AnthropicClient, AuthRoute, AuthSource, ProviderClient};
use base64::Engine as _;
use runtime::session::Session;
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage, ConversationRuntime,
    PermissionMode, PermissionPolicy, RuntimeError, RuntimeFeatureConfig, StaticToolExecutor,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tools::GlobalToolRegistry;

use super::runtime_bridge::tests::NoLoginAnywhere;
use super::runtime_bridge::LiveAsyncApiClient;

/// A model whose catalog row declares the 32 MiB body ceiling.
const MODEL: &str = "claude-opus-5-5";

/// A tool that returns pictures and is in every builtin registry, so the
/// history reaches the wire as tool results and not as archived prose.
const PICTURE_TOOL: &str = "read_image";

const MIB: usize = 1 << 20;

/// The pictures a Computer Use step needs in view: its latest look and the one
/// before it, to compare. Named here as a number of the product's behaviour, so
/// these tests use nothing the change under test adds — they read the same on
/// the code before it.
const NEWEST_TWO: usize = 2;

/// What the endpoint was sent: the size of a body and what it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BodySeen {
    bytes: usize,
    pictures: usize,
    left_out: usize,
}

fn http_reply(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The whole request body, or `None` when the connection dies first. Headers
/// are read a byte at a time (they are a few hundred); the body, which can be
/// tens of megabytes, in one `read_exact`.
async fn read_request_body(socket: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).await.ok()?;
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
    let length: usize = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:")?.trim().parse().ok())?;
    let mut body = vec![0_u8; length];
    socket.read_exact(&mut body).await.ok()?;
    Some(body)
}

/// An endpoint that refuses a body over `limit` bytes with the provider's own
/// 413 and answers anything smaller with a short streamed message. Every body
/// it was sent is kept.
async fn a_body_gate(limit: usize) -> (String, Arc<Mutex<Vec<BodySeen>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the gate");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen: Arc<Mutex<Vec<BodySeen>>> = Arc::default();
    let heard = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let Some(body) = read_request_body(&mut socket).await else {
                continue;
            };
            let text = String::from_utf8_lossy(&body);
            heard.lock().expect("seen").push(BodySeen {
                bytes: body.len(),
                pictures: text.matches("\"type\":\"image\"").count(),
                left_out: text.matches("[picture left out").count(),
            });
            let reply = if body.len() > limit {
                http_reply(
                    "413 Payload Too Large",
                    "application/json",
                    r#"{"type":"error","error":{"type":"request_too_large","message":"Request exceeds the maximum size"}}"#,
                )
            } else {
                let stream = concat!(
                    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_gate\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-opus-5-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"carrying on\"}}\n\n",
                    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}\n\n",
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                );
                http_reply("200 OK", "text/event-stream", stream)
            };
            let _ = socket.write_all(reply.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (base_url, seen)
}

/// A real tiny PNG header padded to `bytes` file bytes: reads as 1280x800 and
/// is `bytes * 4 / 3` characters of base64 on the wire.
fn picture(bytes: usize, salt: u8) -> String {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(1280, 800))
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode the PNG header");
    let mut file = out.into_inner();
    file.resize(bytes.max(file.len()), salt);
    base64::engine::general_purpose::STANDARD.encode(file)
}

/// The person's request, then `steps` looks each answered with a picture of
/// `picture_bytes` file bytes — a Computer Use history, ending on a tool result.
fn history(steps: usize, picture_bytes: usize) -> Vec<ConversationMessage> {
    let mut messages = vec![ConversationMessage::user_text("Check the sync option.")];
    for index in 0..steps {
        let id = format!("toolu_{index:03}");
        messages.push(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: id.clone(),
            name: PICTURE_TOOL.to_string(),
            input: "{\"path\":\"look.png\"}".to_string(),
        }]));
        messages.push(ConversationMessage::tool_result_with_images(
            id,
            PICTURE_TOOL,
            format!("step {index}: screenshot taken"),
            false,
            vec![(
                "image/png".to_string(),
                picture(picture_bytes, u8::try_from(index % 200).expect("salt fits a byte")),
            )],
        ));
    }
    messages
}

fn stored_pictures(messages: &[ConversationMessage]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| &message.blocks)
        .flat_map(|block| match block {
            ContentBlock::ToolResult { images, .. } => {
                images.iter().map(|(_, data)| data.clone()).collect()
            }
            ContentBlock::Image { data, .. } => vec![data.clone()],
            _ => Vec::new(),
        })
        .collect()
}

/// The synchronous client of a streaming turn that goes through the async one:
/// nothing in these tests may reach it.
struct NoSyncClient;

impl ApiClient for NoSyncClient {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Err(RuntimeError::new("this turn goes through the async client"))
    }
}

struct AllowAll;

impl runtime::permission::PermissionPrompter for AllowAll {
    fn decide<'a>(
        &'a self,
        _request: runtime::permission::PermissionRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<runtime::permission::PermissionDecision, runtime::permission::PermissionError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Ok(runtime::permission::PermissionDecision::Allow) })
    }
}

/// What one turn on the live client came to.
struct Outcome {
    /// `Ok(whether the turn compacted)`, or the error the turn ended on.
    turn: Result<bool, String>,
    /// The notices the turn showed the person.
    notices: Vec<String>,
    /// The session's messages after the turn.
    after: Vec<ConversationMessage>,
}

/// One turn over `messages` on the live async client, pointed at `base_url`.
async fn turn_on(base_url: String, messages: Vec<ConversationMessage>) -> Outcome {
    let client = ProviderClient::Anthropic(
        AnthropicClient::from_auth(AuthSource::BearerToken("test-login".to_string()))
            .with_base_url(base_url),
    );
    let live = LiveAsyncApiClient::new(
        client,
        MODEL.to_string(),
        AuthRoute::Auto,
        true,
        None,
        GlobalToolRegistry::builtin(),
        None,
        None,
        None,
    );
    let mut session = Session::new();
    session.messages = Arc::new(messages);
    let features = RuntimeFeatureConfig::default().with_auto_dream_enabled(false);
    let mut runtime = ConversationRuntime::new_with_features(
        session,
        NoSyncClient,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
        &features,
    )
    .with_async_api_client(Arc::new(live));
    // The window the person's model has: the turn is nowhere near it.
    runtime.set_context_window(1_000_000);
    let (tx, mut rx) = mpsc::channel(runtime::DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut notices = Vec::new();
        while let Some(block) = rx.recv().await {
            if let runtime::message_stream::RenderBlock::System { text, .. } = block {
                notices.push(text);
            }
        }
        notices
    });
    let prompter: Arc<dyn runtime::permission::PermissionPrompter> = Arc::new(AllowAll);
    let turn = runtime
        .run_turn_streaming("keep going", tx, prompter)
        .await
        .map(|summary| summary.auto_compaction.is_some())
        .map_err(|error| error.to_string());
    let notices = drain.await.expect("drain");
    Outcome {
        turn,
        notices,
        after: runtime.session().messages.as_ref().clone(),
    }
}

/// A world with no login in it and the built-in model catalog: nothing of the
/// person's own is read, and no catalog override changes the ceiling.
fn isolated() -> (NoLoginAnywhere, crate::support::EnvVarGuard) {
    (
        NoLoginAnywhere::new(),
        crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None),
    )
}

fn tokio_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

/// The report: thirty looks of ~2 MiB each on the wire (60 MiB of pictures, 50k
/// tokens) on a 1M window. The compaction tail alone holds ~24 of them, more
/// than the ceiling, so compaction cannot bring the body under it. The client's
/// own preflight, reading the catalog's `max_request_bytes`, refuses the body;
/// the turn must go on instead of ending on that refusal.
#[test]
fn a_turn_whose_kept_tail_of_screenshots_passes_the_ceiling_goes_on() {
    let _lock = crate::test_env_lock();
    let _world = isolated();
    let ceiling = api::max_request_bytes_for_model(MODEL).expect("the catalog declares a ceiling");
    tokio_runtime().block_on(async {
        // The endpoint takes anything: only the client's preflight can refuse.
        let (base_url, seen) = a_body_gate(usize::MAX).await;
        let before = history(30, MIB * 3 / 2);
        let outcome = turn_on(base_url, before.clone()).await;

        assert!(
            outcome.turn.is_ok(),
            "the turn ended on the refusal instead of going on: {:?}",
            outcome.turn
        );
        assert_eq!(outcome.turn, Ok(false), "no summary was written for a picture problem");
        let seen = seen.lock().expect("seen").clone();
        assert_eq!(seen.len(), 1, "one request, sent whole: {seen:?}");
        let body = &seen[0];
        assert!(
            u64::try_from(body.bytes).expect("fits") <= ceiling,
            "the request is under the ceiling the client refuses against: {body:?} vs {ceiling}"
        );
        assert_eq!(body.pictures + body.left_out, 30, "every look is a picture or a note: {body:?}");
        assert!(body.left_out > 0, "older pictures are notes: {body:?}");
        assert!(body.pictures >= NEWEST_TWO, "the newest are pictures: {body:?}");
        // Nothing stored was rewritten.
        assert_eq!(
            stored_pictures(&outcome.after[..before.len()]),
            stored_pictures(&before),
            "the session still holds every picture it was given"
        );
        assert_eq!(&outcome.after[..before.len()], &before[..], "and every other message");
    });
}

/// The same recovery when the REFUSAL comes from the wire: an endpoint (a
/// gateway, a proxy, a ceiling the catalog does not know) answers 413 for a body
/// the catalog calls fine. The turn leaves the oldest pictures out and sends
/// again — newest two first, newest one if that is still too much — and goes on.
#[test]
fn a_413_from_the_wire_is_answered_by_leaving_pictures_out_and_the_turn_goes_on() {
    let _lock = crate::test_env_lock();
    let _world = isolated();
    tokio_runtime().block_on(async {
        // 2.5 MiB: below two pictures of 1.33 MB on the wire, above one — with
        // room to spare for the system prompt and the tool definitions.
        let (base_url, seen) = a_body_gate(MIB * 5 / 2).await;
        let before = history(8, 1_000_000);
        let outcome = turn_on(base_url, before.clone()).await;

        assert!(
            outcome.turn.is_ok(),
            "the turn ended on the provider's 413 instead of going on: {:?}",
            outcome.turn
        );
        assert_eq!(outcome.turn, Ok(false), "no summary was written for a picture problem");
        let seen = seen.lock().expect("seen").clone();
        let pictures: Vec<usize> = seen.iter().map(|body| body.pictures).collect();
        assert_eq!(
            pictures,
            [8, NEWEST_TWO, 1],
            "all of them, refused; the newest two, refused; the newest one, accepted: {seen:?}"
        );
        assert_eq!(seen[2].left_out, 7, "{seen:?}");
        assert!(seen[2].bytes <= MIB * 5 / 2, "{seen:?}");
        assert!(
            outcome.notices.iter().any(|text| text.contains("too large")),
            "the person is told what is being done: {:?}",
            outcome.notices
        );
        assert_eq!(
            stored_pictures(&outcome.after[..before.len()]),
            stored_pictures(&before),
            "the session still holds every picture it was given"
        );
    });
}

/// And when no picture can be the way out the turn ends — saying which picture
/// is the newest and how big the rest of the request is, not just that the body
/// was too large.
#[test]
fn when_even_the_newest_picture_is_refused_the_message_names_it() {
    let _lock = crate::test_env_lock();
    let _world = isolated();
    tokio_runtime().block_on(async {
        // Below one picture.
        let (base_url, seen) = a_body_gate(100_000).await;
        let outcome = turn_on(base_url, history(8, 1_000_000)).await;
        assert!(outcome.turn.is_err(), "no request can fit under 100 KB: {:?}", outcome.turn);
        let error = outcome.turn.expect_err("checked above");
        assert!(error.contains("413"), "the provider's own words stay: {error}");
        assert!(error.contains("Pictures cannot be cut further"), "{error}");
        assert!(error.contains(&format!("the {PICTURE_TOOL} result at message")), "which: {error}");
        assert!(error.contains("image/png 1280x800"), "what: {error}");
        assert!(error.contains("on the wire"), "how big: {error}");
        let pictures: Vec<usize> =
            seen.lock().expect("seen").iter().take(3).map(|body| body.pictures).collect();
        assert_eq!(pictures, [8, NEWEST_TWO, 1], "the ladder, before anything else");
    });
}
