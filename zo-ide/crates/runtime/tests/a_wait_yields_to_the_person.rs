//! A wait yields to the person (t-11354).
//!
//! The `Sleep` tool is a wait and nothing else: it holds the turn so the
//! model can look again later. A person who types while it holds should not
//! wait behind it — their words are what the model looks at next. The wait
//! ends when a steer is pending, the tool reports the time it actually
//! waited, and the steer is folded at that boundary.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use runtime::message_stream::types::BlockId;
use runtime::message_stream::RenderBlock;
use runtime::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError, PermissionPrompter,
    PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::Session;
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ContentBlock, ConversationRuntime,
    PermissionMode, PermissionPolicy, RuntimeError, SteeringQueue, StaticToolExecutor,
    DEFAULT_STREAMING_CHANNEL_CAPACITY,
};
use tokio::sync::mpsc;

/// What the person types while the wait holds.
const STEER: &str = "현재 상황";

/// The wait the model asks for: the longest the tool grants.
const ASKED_WAIT_MS: u64 = 5_000;

/// When the person types, after the model's call that asked for the wait.
const TYPED_AFTER: Duration = Duration::from_millis(300);

struct ExplodingSyncApi;

impl ApiClient for ExplodingSyncApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        panic!("sync ApiClient::stream must not be called when the async seam is installed");
    }
}

struct DenyPrompter;

impl PermissionPrompter for DenyPrompter {
    fn decide<'a>(
        &'a self,
        _request: AsyncPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>>
    {
        Box::pin(async { Ok(AsyncPermissionDecision::Deny) })
    }
}

/// First call: ask for the wait, and have the person type [`TYPED_AFTER`]
/// later. Second call: answer, keeping the request for the assertions.
struct WaitThenAnswer {
    calls: AtomicUsize,
    steering: SteeringQueue,
    answered_request: Arc<Mutex<Option<ApiRequest>>>,
}

impl AsyncApiClient for WaitThenAnswer {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let steering = Arc::clone(&self.steering);
                tokio::spawn(async move {
                    tokio::time::sleep(TYPED_AFTER).await;
                    steering
                        .lock()
                        .expect("steering queue")
                        .push(STEER.to_string());
                });
                return Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "wait-1".to_string(),
                        name: "Sleep".to_string(),
                        input: format!("{{\"duration_ms\":{ASKED_WAIT_MS}}}"),
                    },
                    AssistantEvent::MessageStop,
                ]);
            }
            *self.answered_request.lock().expect("request") = Some(request);
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: "지금 상황입니다.".to_string(),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            Ok(vec![
                AssistantEvent::TextDelta("지금 상황입니다.".to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

#[tokio::test]
async fn a_sleep_ends_when_the_person_types_and_their_words_are_read_next() {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new().register("Sleep", |input| Ok(input.to_string())),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    let answered_request = Arc::new(Mutex::new(None));
    runtime.set_async_api_client(Arc::new(WaitThenAnswer {
        calls: AtomicUsize::new(0),
        steering: runtime.steering_handle(),
        answered_request: Arc::clone(&answered_request),
    }));

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let started = Instant::now();
    runtime
        .run_turn_streaming("기다려", tx, prompter)
        .await
        .expect("the turn runs");
    let took = started.elapsed();
    drain.await.expect("drain");

    assert!(
        took < Duration::from_millis(ASKED_WAIT_MS / 2),
        "the wait held the person's words for {took:?}"
    );
    let request = answered_request
        .lock()
        .expect("request")
        .take()
        .expect("the model was asked again");
    let texts: Vec<&str> = request
        .messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::ToolResult { output, .. } => Some(output.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        texts.iter().any(|text| text.contains(STEER)),
        "the next request carries the person's words: {texts:?}"
    );
    let waited = texts
        .iter()
        .find(|text| text.contains("duration_ms"))
        .expect("the wait's result");
    assert!(
        !waited.contains(&format!("\"duration_ms\":{ASKED_WAIT_MS}")),
        "the wait reports the time it waited, not the time it was asked for: {waited}"
    );
}
