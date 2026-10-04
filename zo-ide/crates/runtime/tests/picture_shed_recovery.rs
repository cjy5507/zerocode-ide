//! t-37798 — a provider that refuses a request body as too large is answered by
//! leaving the older pictures out and sending again, inside the SAME turn.
//!
//! The wall these tests stand at is a gateway (or a ceiling the catalog does not
//! know) answering `413 request_too_large`. The turn used to compact once —
//! which keeps the recent tail, pictures and all — and end on the second 413
//! with most of its context window free. The fake gateway below lowers each
//! request exactly as the production clients do (`WireTarget` with the
//! request's picture cap) and refuses a body over its own limit, so what it
//! sees is what a real provider would be sent.
//!
//! Covered paths: the synchronous turn, the streaming turn on a synchronous
//! client, and the streaming turn on an async client — three loops that each
//! answer a refusal.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use api::{InputContentBlock, ProviderErrorClass, ToolResultContentBlock};
use base64::Engine as _;
use runtime::message_stream::types::BlockId;
use runtime::message_stream::RenderBlock;
use runtime::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError, PermissionPrompter,
    PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::Session;
use runtime::{
    convert_messages_for, ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ContentBlock,
    ConversationMessage, ConversationRuntime, PermissionMode, PermissionPolicy, RuntimeError,
    RuntimeFeatureConfig, StaticToolExecutor, WireTarget, KEEP_NEWEST_PICTURES,
};
use tokio::sync::mpsc;

/// A model whose catalog row declares the 32 MiB ceiling — far above the
/// gateway's own, so only the REFUSAL can tell the runtime about this limit.
const MODEL: &str = "claude-opus-5-5";

/// Bytes of the gateway's own body limit: below ten pictures, above two.
const GATEWAY_LIMIT: usize = 600_000;

/// File bytes of one synthetic screenshot; 120,000 characters of base64.
const PICTURE_BYTES: usize = 90_000;

fn picture(salt: u8) -> String {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(1280, 800))
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode the PNG header");
    let mut file = out.into_inner();
    file.resize(PICTURE_BYTES, salt);
    base64::engine::general_purpose::STANDARD.encode(file)
}

/// The person's request, then `steps` Computer Use looks, each answered with a
/// picture. Ends on a tool result, as a history does when the model's next
/// request is about to go out.
fn history(steps: usize) -> Vec<ConversationMessage> {
    let mut messages = vec![ConversationMessage::user_text("Check the sync option.")];
    for index in 0..steps {
        let id = format!("toolu_{index:03}");
        messages.push(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: id.clone(),
            name: "Computer".to_string(),
            input: "{\"action\":\"screenshot\"}".to_string(),
        }]));
        messages.push(ConversationMessage::tool_result_with_images(
            id,
            "Computer",
            format!("step {index}: screenshot taken"),
            false,
            vec![(
                "image/png".to_string(),
                picture(u8::try_from(index % 200).expect("salt fits a byte")),
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

/// What the gateway was sent, request by request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    bytes: usize,
    pictures_on_the_wire: usize,
    pictures_left_out: usize,
    cap: Option<usize>,
}

#[derive(Clone)]
struct Gateway {
    limit: usize,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Gateway {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            seen: Arc::default(),
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("seen").clone()
    }

    fn answer(&self, request: &ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        // Lowered the way the production clients lower it.
        let lowered = convert_messages_for(
            &request.messages,
            WireTarget::for_model(MODEL).with_picture_cap(request.picture_cap),
        );
        let bytes = serde_json::to_vec(&lowered).expect("serialize the request").len();
        let (mut on_the_wire, mut left_out) = (0, 0);
        for block in lowered.iter().flat_map(|message| &message.content) {
            if let InputContentBlock::ToolResult { content, .. } = block {
                for part in content {
                    match part {
                        ToolResultContentBlock::Image { .. } => on_the_wire += 1,
                        ToolResultContentBlock::Text { text }
                            if text.starts_with("[picture left out") =>
                        {
                            left_out += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        self.seen.lock().expect("seen").push(Seen {
            bytes,
            pictures_on_the_wire: on_the_wire,
            pictures_left_out: left_out,
            cap: request.picture_cap,
        });
        if bytes > self.limit {
            return Err(RuntimeError::with_provider_error_class(
                format!(
                    "api returned 413 Payload Too Large (request_too_large): Request exceeds \
                     the maximum size: {bytes} bytes serialized, endpoint accepts {}",
                    self.limit
                ),
                ProviderErrorClass::ContextOverflow,
            ));
        }
        Ok(vec![
            AssistantEvent::TextDelta("on we go".to_string()),
            AssistantEvent::MessageStop,
        ])
    }
}

impl ApiClient for Gateway {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.answer(&request)
    }
}

impl AsyncApiClient for Gateway {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        let answer = self.answer(&request);
        Box::pin(async move { answer })
    }
}

struct AllowAll;

impl PermissionPrompter for AllowAll {
    fn decide<'a>(
        &'a self,
        _request: AsyncPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>>
    {
        Box::pin(async { Ok(AsyncPermissionDecision::Allow) })
    }
}

fn runtime_over(
    messages: Vec<ConversationMessage>,
    gateway: &Gateway,
) -> ConversationRuntime<Gateway, StaticToolExecutor> {
    let mut session = Session::new();
    session.messages = Arc::new(messages);
    let features = RuntimeFeatureConfig::default().with_auto_dream_enabled(false);
    let mut runtime = ConversationRuntime::new_with_features(
        session,
        gateway.clone(),
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
        &features,
    );
    // The window the person's model has: the turn is nowhere near it.
    runtime.set_context_window(1_000_000);
    runtime.set_auto_compaction_enabled(false);
    runtime
}

/// What a recovered turn must show, whichever loop ran it.
fn assert_recovered(gateway: &Gateway, before: &[ConversationMessage], after: &Session) {
    let seen = gateway.seen();
    assert_eq!(seen.len(), 2, "refused once, accepted once: {seen:?}");
    assert_eq!(seen[0].cap, None, "the first request goes out as built");
    assert_eq!(seen[0].pictures_on_the_wire, 10, "{seen:?}");
    assert!(seen[0].bytes > GATEWAY_LIMIT, "the gateway refused it: {seen:?}");
    assert_eq!(seen[1].cap, Some(KEEP_NEWEST_PICTURES), "{seen:?}");
    assert_eq!(seen[1].pictures_on_the_wire, KEEP_NEWEST_PICTURES, "{seen:?}");
    assert_eq!(seen[1].pictures_left_out, 8, "each older picture is one note: {seen:?}");
    assert!(seen[1].bytes <= GATEWAY_LIMIT, "and now it fits: {seen:?}");
    // Nothing stored was rewritten: the pictures are all still in the session.
    assert_eq!(
        stored_pictures(&after.messages[..before.len()]),
        stored_pictures(before),
        "the retry left the stored pictures alone"
    );
    assert_eq!(&after.messages[..before.len()], before, "and everything else");
    assert!(
        matches!(
            after.messages.last(),
            Some(message) if message.blocks.iter().any(|block| {
                matches!(block, ContentBlock::Text { text } if text == "on we go")
            })
        ),
        "the turn went on and the model answered"
    );
}

#[test]
fn a_refused_body_leaves_older_pictures_out_and_the_turn_goes_on() {
    let gateway = Gateway::new(GATEWAY_LIMIT);
    let before = history(10);
    let mut runtime = runtime_over(before.clone(), &gateway);
    let turn = runtime.run_turn("keep going", None);
    assert!(
        turn.is_ok(),
        "a body that is too large is answered, not surfaced: {:?}",
        turn.as_ref().err()
    );
    let summary = turn.expect("checked above");
    assert!(
        summary.auto_compaction.is_none(),
        "no summary is written for a problem pictures solve"
    );
    assert_recovered(&gateway, &before, runtime.session());
}

#[tokio::test]
async fn the_streaming_turn_on_a_synchronous_client_recovers_the_same_way() {
    let gateway = Gateway::new(GATEWAY_LIMIT);
    let before = history(10);
    let mut runtime = runtime_over(before.clone(), &gateway);
    let (tx, mut rx) = mpsc::channel(runtime::DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(AllowAll);
    let turn = runtime.run_turn_streaming("keep going", tx, prompter).await;
    drain.await.expect("drain");
    assert!(
        turn.is_ok(),
        "a body that is too large is answered, not surfaced: {:?}",
        turn.as_ref().err().map(ToString::to_string)
    );
    assert!(turn.expect("checked above").auto_compaction.is_none());
    assert_recovered(&gateway, &before, runtime.session());
}

#[tokio::test]
async fn the_streaming_turn_on_an_async_client_recovers_the_same_way() {
    let gateway = Gateway::new(GATEWAY_LIMIT);
    let before = history(10);
    let mut runtime =
        runtime_over(before.clone(), &gateway).with_async_api_client(Arc::new(gateway.clone()));
    let (tx, mut rx) = mpsc::channel(runtime::DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut notices = Vec::new();
        while let Some(block) = rx.recv().await {
            if let RenderBlock::System { text, .. } = block {
                notices.push(text);
            }
        }
        notices
    });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(AllowAll);
    let turn = runtime.run_turn_streaming("keep going", tx, prompter).await;
    let notices = drain.await.expect("drain");
    assert!(
        turn.is_ok(),
        "a body that is too large is answered, not surfaced: {:?}",
        turn.as_ref().err().map(ToString::to_string)
    );
    assert!(turn.expect("checked above").auto_compaction.is_none());
    assert_recovered(&gateway, &before, runtime.session());
    assert!(
        notices.iter().any(|text| text.contains("too large") && text.contains("newest 2 pictures")),
        "the person is told what is being done: {notices:?}"
    );
}

#[test]
fn the_refused_pictures_come_back_when_the_next_turn_starts() {
    let gateway = Gateway::new(GATEWAY_LIMIT);
    let mut runtime = runtime_over(history(10), &gateway);
    let first = runtime.run_turn("first", None);
    assert!(first.is_ok(), "the first turn recovers: {:?}", first.as_ref().err());
    let second = runtime.run_turn("second", None);
    assert!(second.is_ok(), "and so does the second: {:?}", second.as_ref().err());
    let seen = gateway.seen();
    // Turn two starts on the catalog's budget again, not on the cap turn one
    // ended with: nothing here says the gateway still refuses it.
    assert_eq!(seen[2].cap, None, "{seen:?}");
    assert_eq!(seen[2].pictures_on_the_wire, 10, "{seen:?}");
    assert_eq!(seen[3].cap, Some(KEEP_NEWEST_PICTURES), "{seen:?}");
}

#[test]
fn when_even_one_picture_is_refused_the_turn_says_which_and_how_big() {
    // A limit below one picture: pictures cannot be the way out.
    let gateway = Gateway::new(100_000);
    let mut runtime = runtime_over(history(10), &gateway);
    let turn = runtime.run_turn("keep going", None);
    assert!(turn.is_err(), "nothing can make this request fit");
    let error = turn.expect_err("checked above");
    let seen = gateway.seen();
    let caps: Vec<Option<usize>> = seen.iter().take(3).map(|request| request.cap).collect();
    assert_eq!(
        caps,
        [None, Some(KEEP_NEWEST_PICTURES), Some(1)],
        "the ladder goes newest-two, then newest-one, before anything else: {seen:?}"
    );
    assert_eq!(error.provider_error_class(), Some(ProviderErrorClass::ContextOverflow));
    let text = error.to_string();
    assert!(text.contains("413"), "the provider's own words stay: {text}");
    assert!(text.contains("Pictures cannot be cut further"), "{text}");
    assert!(text.contains("the Computer result at message"), "which picture: {text}");
    assert!(text.contains("image/png 1280x800"), "what it is: {text}");
    assert!(text.contains("on the wire"), "how big: {text}");
}
