//! Mid-generation steering re-issue ("gen-abort") on the streaming dispatcher.
//!
//! Steering used to be delivered only at a tool-result boundary, so a steer
//! typed while the model was still *generating* waited for that call to finish
//! AND for its whole tool batch to execute — the user watched the agent edit
//! the very file they had just excluded. These tests pin the earlier delivery:
//! a call that has not started emitting `tool_use` yet is abandoned, the
//! steering is folded with the existing wire shape, and the request is
//! re-issued with the identical configuration.
//!
//! Every scripted client here stalls with real sleeps after enqueueing its
//! steer, because a call can only be abandoned while the provider future is
//! pending. The stall is bounded and sets a completion flag on the way out, so
//! a regression that stops abandoning surfaces as a failed assertion rather
//! than a hung test.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use runtime::message_stream::types::{BlockId, ToolCallId, ToolCallStatus, ToolPreview};
use runtime::message_stream::RenderBlock;
use runtime::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError, PermissionPrompter,
    PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::{MessageRole, Session};
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ContentBlock, ConversationMessage,
    ConversationRuntime, PermissionMode, PermissionPolicy, RuntimeError, StaticToolExecutor,
    DEFAULT_STREAMING_CHANNEL_CAPACITY,
};
use tokio::sync::mpsc;

/// Upper bound on how long a scripted "still generating" call stalls before it
/// gives up and completes. Comfortably longer than the runtime's silent-stream
/// poll interval, short enough that a broken build fails fast.
const SCRIPTED_GENERATION_STALL: std::time::Duration = std::time::Duration::from_millis(1_500);

/// Sleep in small slices so a dropped future stops promptly.
async fn stall_as_if_generating() {
    let deadline = std::time::Instant::now() + SCRIPTED_GENERATION_STALL;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// Sync client that must never be reached: these tests all run the async
/// streaming dispatcher.
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

fn test_runtime(
    executor: StaticToolExecutor,
) -> ConversationRuntime<ExplodingSyncApi, StaticToolExecutor> {
    ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        executor,
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    )
}

/// All `Text` block content of a message, joined — the shape every steering
/// assertion below reads.
fn message_text(message: &ConversationMessage) -> String {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wire-shape invariants the transcript must satisfy at every point a steering
/// fold can touch it: every `tool_use` is answered, every `tool_result` answers
/// something, and no two assistant turns are adjacent.
///
/// The "must not end on an assistant turn" rule belongs to a *request*, not to
/// a settled session (a completed turn ends on the model's reply by
/// definition), so it lives in [`assert_request_shape`].
fn assert_wire_valid(messages: &[ConversationMessage], context: &str) {
    let mut opened: Vec<String> = Vec::new();
    let mut answered: Vec<String> = Vec::new();
    let mut previous_was_assistant = false;
    for message in messages {
        let is_assistant = message.role == MessageRole::Assistant;
        assert!(
            !(is_assistant && previous_was_assistant),
            "{context}: two adjacent assistant messages"
        );
        previous_was_assistant = is_assistant;
        for block in &message.blocks {
            match block {
                ContentBlock::ToolUse { id, .. } => opened.push(id.clone()),
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    assert!(
                        opened.contains(tool_use_id),
                        "{context}: tool_result {tool_use_id} answers no tool_use"
                    );
                    answered.push(tool_use_id.clone());
                }
                _ => {}
            }
        }
    }
    for id in &opened {
        assert!(
            answered.contains(id),
            "{context}: tool_use {id} was never answered"
        );
    }
}

/// [`assert_wire_valid`] plus the rule that only applies to an outgoing
/// request: it must not end on an assistant turn, or there is nothing for the
/// model to answer.
fn assert_request_shape(messages: &[ConversationMessage], context: &str) {
    assert_wire_valid(messages, context);
    assert_ne!(
        messages.last().expect("a message").role,
        MessageRole::Assistant,
        "{context}: request ends on an assistant turn"
    );
}

async fn collect_blocks(mut rx: mpsc::Receiver<RenderBlock>) -> Vec<RenderBlock> {
    let mut blocks = Vec::new();
    while let Some(block) = rx.recv().await {
        blocks.push(block);
    }
    blocks
}

// ── Harness attestation ─────────────────────────────────────────────────────
//
// The re-issue is the kind of feature the attest ledger exists for: when it
// works it settles NOTHING (no assistant message, no iteration record), so a
// build where it silently stopped firing produces the same transcript as a
// build where it works. `/smart doctor` and `/refine` read the ledger, so a
// firing that is never recorded reads as a dead feature.

/// The ledger is PROCESS-wide by design and every test in this binary shares
/// it, so an exact-delta assertion would be a coin flip under cargo's default
/// parallelism — a sibling test's re-issue would land inside this one's
/// measurement. Every test here that FIRES the feature or MEASURES it takes
/// this lock, which makes each delta the measuring test's own work.
///
/// An ASYNC mutex on purpose: the guard is held across the turn's awaits,
/// because the turn IS the measured region. It also has no poisoning, so one
/// failing test cannot cascade into its siblings and hide which one broke.
static ATTEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Re-issue firings recorded in this process so far.
fn reissue_firings() -> u64 {
    telemetry::harness_attest_snapshot()
        .attestation(telemetry::HarnessFeature::SteeringReissue)
        .fired
}

// ============================================================================
// (a) abandon → fold → re-issue
// ============================================================================

struct SteeredMidGenerationAsyncApi {
    calls: AtomicUsize,
    steering: runtime::SteeringQueue,
    first_call_ran_to_completion: Arc<AtomicBool>,
    reissued_request: Arc<Mutex<Option<Vec<ConversationMessage>>>>,
}

impl AsyncApiClient for SteeredMidGenerationAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                // The model has begun answering the ORIGINAL request…
                render_tx
                    .send(RenderBlock::TextDelta {
                        id: text_block_id,
                        text: "I'll start by rewriting config.rs".to_string(),
                        done: false,
                    })
                    .await
                    .map_err(|_| RuntimeError::new("channel closed"))?;
                // …and the user types a correction while it is still going.
                self.steering
                    .lock()
                    .expect("steering lock")
                    .push("don't touch config.rs".to_string());
                stall_as_if_generating().await;
                // Only reached if the runtime did NOT abandon the call.
                self.first_call_ran_to_completion
                    .store(true, Ordering::SeqCst);
                return Ok(vec![
                    AssistantEvent::TextDelta("\u{2026}and here is the rewrite".to_string()),
                    AssistantEvent::MessageStop,
                ]);
            }
            *self.reissued_request.lock().expect("request slot") = Some(request.messages.to_vec());
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: "leaving config.rs alone".to_string(),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            Ok(vec![
                AssistantEvent::TextDelta("leaving config.rs alone".to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

#[tokio::test]
async fn steering_during_generation_abandons_the_call_and_reissues_with_it_folded() {
    let _serial = ATTEST_SERIAL.lock().await;
    let firings_before = reissue_firings();
    let mut runtime = test_runtime(StaticToolExecutor::new());
    let steering = runtime.steering_handle();
    let first_call_ran_to_completion = Arc::new(AtomicBool::new(false));
    let reissued_request = Arc::new(Mutex::new(None));
    let client = Arc::new(SteeredMidGenerationAsyncApi {
        calls: AtomicUsize::new(0),
        steering: steering.clone(),
        first_call_ran_to_completion: Arc::clone(&first_call_ran_to_completion),
        reissued_request: Arc::clone(&reissued_request),
    });
    runtime.set_async_api_client(Arc::clone(&client) as Arc<dyn AsyncApiClient>);

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(collect_blocks(rx));
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("rewrite the config", tx, prompter)
        .await
        .expect("the re-issued turn should complete");
    let blocks = drain.await.expect("drain");

    assert!(
        !first_call_ran_to_completion.load(Ordering::SeqCst),
        "the in-flight call must be abandoned, not awaited to completion"
    );
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        2,
        "the abandoned call must be re-issued exactly once"
    );

    // Nothing from the abandoned generation settled: the only assistant
    // message in the session is the re-issued call's reply.
    let assistant_text = runtime
        .session()
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(message_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        assistant_text, "leaving config.rs alone",
        "the abandoned partial must not be settled into history"
    );
    assert_eq!(summary.assistant_messages.len(), 1);

    // The re-issued request carried the steering, on a wire-valid history.
    let reissued = reissued_request
        .lock()
        .expect("request slot")
        .clone()
        .expect("the re-issued call recorded its request");
    assert_request_shape(&reissued, "re-issued request");
    let tail = message_text(reissued.last().expect("a message"));
    assert!(
        tail.contains("don't touch config.rs") && tail.contains("[User steering"),
        "the re-issued request should carry the steering with its preamble, got {tail:?}"
    );

    // Exactly one echo, emitted where the fold happened.
    let echoes = blocks
        .iter()
        .filter(|block| {
            matches!(
                block,
                RenderBlock::System { text, .. }
                    if text.contains("\u{2937} steering: don't touch config.rs")
            )
        })
        .count();
    assert_eq!(
        echoes, 1,
        "expected exactly one steering echo, got {blocks:?}"
    );
    assert!(
        steering.lock().expect("steering lock").is_empty(),
        "the queue should be drained by the fold"
    );
    assert_wire_valid(&runtime.session().messages, "post-turn session");

    // The ledger row is the only durable evidence this ran: everything else
    // asserted above is invisible once the turn is over.
    assert_eq!(
        reissue_firings() - firings_before,
        1,
        "the re-issue must attest exactly one firing"
    );
}

// ============================================================================
// (b) cap: one abandonment per boundary
// ============================================================================

struct SteeredTwiceAsyncApi {
    calls: AtomicUsize,
    steering: runtime::SteeringQueue,
    second_call_ran_to_completion: Arc<AtomicBool>,
    third_call_tail: Arc<Mutex<Option<String>>>,
}

impl AsyncApiClient for SteeredTwiceAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            match call {
                0 => {
                    self.steering
                        .lock()
                        .expect("steering lock")
                        .push("first steer".to_string());
                    stall_as_if_generating().await;
                }
                1 => {
                    let tail = message_text(request.messages.last().expect("a message"));
                    assert!(
                        tail.contains("first steer"),
                        "the re-issued call should carry the first steer, got {tail:?}"
                    );
                    // A second correction lands while the RE-ISSUED call is
                    // generating. The cap must hold: no second abandonment.
                    self.steering
                        .lock()
                        .expect("steering lock")
                        .push("second steer".to_string());
                    stall_as_if_generating().await;
                    self.second_call_ran_to_completion
                        .store(true, Ordering::SeqCst);
                }
                _ => {
                    *self.third_call_tail.lock().expect("tail slot") =
                        Some(message_text(request.messages.last().expect("a message")));
                }
            }
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: format!("reply {call}"),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            Ok(vec![
                AssistantEvent::TextDelta(format!("reply {call}")),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

#[tokio::test]
async fn a_reissued_call_is_not_abandoned_again_by_a_second_steer() {
    let _serial = ATTEST_SERIAL.lock().await;
    let firings_before = reissue_firings();
    let mut runtime = test_runtime(StaticToolExecutor::new());
    let steering = runtime.steering_handle();
    let second_call_ran_to_completion = Arc::new(AtomicBool::new(false));
    let third_call_tail = Arc::new(Mutex::new(None));
    let client = Arc::new(SteeredTwiceAsyncApi {
        calls: AtomicUsize::new(0),
        steering: steering.clone(),
        second_call_ran_to_completion: Arc::clone(&second_call_ran_to_completion),
        third_call_tail: Arc::clone(&third_call_tail),
    });
    runtime.set_async_api_client(Arc::clone(&client) as Arc<dyn AsyncApiClient>);

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(collect_blocks(rx));
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime
        .run_turn_streaming("start", tx, prompter)
        .await
        .expect("the turn should complete");
    drain.await.expect("drain");

    assert!(
        second_call_ran_to_completion.load(Ordering::SeqCst),
        "the re-issued call must run to completion — one abandonment per boundary"
    );
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        3,
        "abandon, re-issue, then one boundary-folded continuation"
    );
    let tail = third_call_tail
        .lock()
        .expect("tail slot")
        .clone()
        .expect("the third call recorded its tail");
    assert!(
        tail.contains("second steer"),
        "the second steer should arrive via the ordinary boundary fold, got {tail:?}"
    );
    assert!(steering.lock().expect("steering lock").is_empty());
    assert_wire_valid(&runtime.session().messages, "post-turn session");

    // The cap is visible in the ledger too: two steers, one abandonment, one
    // firing. A count of 2 here would mean the boundary cap had come off.
    assert_eq!(
        reissue_firings() - firings_before,
        1,
        "one abandonment per boundary must attest one firing, not one per steer"
    );
}

// ============================================================================
// (c) negative control: an empty queue changes nothing
// ============================================================================

struct SlowButUnsteeredAsyncApi {
    calls: AtomicUsize,
    ran_to_completion: Arc<AtomicBool>,
}

impl AsyncApiClient for SlowButUnsteeredAsyncApi {
    fn stream_async<'a>(
        &'a self,
        _request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: "thinking".to_string(),
                    done: false,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            stall_as_if_generating().await;
            self.ran_to_completion.store(true, Ordering::SeqCst);
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: " done".to_string(),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            Ok(vec![
                AssistantEvent::TextDelta("thinking done".to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

#[tokio::test]
async fn an_empty_steering_queue_leaves_a_slow_generation_untouched() {
    let _serial = ATTEST_SERIAL.lock().await;
    let firings_before = reissue_firings();
    let mut runtime = test_runtime(StaticToolExecutor::new());
    let ran_to_completion = Arc::new(AtomicBool::new(false));
    let client = Arc::new(SlowButUnsteeredAsyncApi {
        calls: AtomicUsize::new(0),
        ran_to_completion: Arc::clone(&ran_to_completion),
    });
    runtime.set_async_api_client(Arc::clone(&client) as Arc<dyn AsyncApiClient>);

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(collect_blocks(rx));
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("the unsteered turn should complete");
    let blocks = drain.await.expect("drain");

    assert!(
        ran_to_completion.load(Ordering::SeqCst),
        "an empty queue must never abandon a call"
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
    assert_eq!(summary.iterations, 1);
    assert!(
        !blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { text, .. } if text.contains("\u{2937} steering:")
        )),
        "no steering echo should be emitted, got {blocks:?}"
    );
    // Both deltas reached the consumer, in order, through the probe interposer.
    let text: String = blocks
        .iter()
        .filter_map(|block| match block {
            RenderBlock::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "thinking done", "got {blocks:?}");

    // The negative half of the ledger claim: a counter that also ticks when
    // nothing was abandoned proves nothing about the feature at all.
    assert_eq!(
        reissue_firings(),
        firings_before,
        "an untouched generation must attest no firing"
    );
}

// ============================================================================
// (d) a call that already emitted a tool_use keeps the boundary path
// ============================================================================

struct SteeredAfterToolUseAsyncApi {
    calls: AtomicUsize,
    steering: runtime::SteeringQueue,
    first_call_ran_to_completion: Arc<AtomicBool>,
    second_call_tail: Arc<Mutex<Option<ConversationMessage>>>,
}

impl AsyncApiClient for SteeredAfterToolUseAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call > 0 {
                *self.second_call_tail.lock().expect("tail slot") =
                    request.messages.last().cloned();
                render_tx
                    .send(RenderBlock::TextDelta {
                        id: text_block_id,
                        text: "acknowledged".to_string(),
                        done: true,
                    })
                    .await
                    .map_err(|_| RuntimeError::new("channel closed"))?;
                return Ok(vec![
                    AssistantEvent::TextDelta("acknowledged".to_string()),
                    AssistantEvent::MessageStop,
                ]);
            }
            // The tool_use block has started streaming — this is the moment
            // after which abandonment is off the table.
            render_tx
                .send(RenderBlock::ToolCall {
                    id: text_block_id,
                    tool_call_id: ToolCallId("call-1".to_string()),
                    name: "echo".to_string(),
                    summary: "echo hi".to_string(),
                    preview: ToolPreview::Generic {
                        name: "echo".to_string(),
                        input_summary: "hi".to_string(),
                    },
                    status: ToolCallStatus::Pending,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            self.steering
                .lock()
                .expect("steering lock")
                .push("stop after this".to_string());
            stall_as_if_generating().await;
            self.first_call_ran_to_completion
                .store(true, Ordering::SeqCst);
            Ok(vec![
                AssistantEvent::ToolUse {
                    id: "call-1".to_string(),
                    name: "echo".to_string(),
                    input: "{}".to_string(),
                },
                AssistantEvent::MessageStop,
            ])
        })
    }
}

#[tokio::test]
async fn a_call_that_already_emitted_a_tool_use_is_not_abandoned() {
    let _serial = ATTEST_SERIAL.lock().await;
    let firings_before = reissue_firings();
    let mut runtime = test_runtime(
        StaticToolExecutor::new().register("echo", |_input| Ok("echoed".to_string())),
    );
    let steering = runtime.steering_handle();
    let first_call_ran_to_completion = Arc::new(AtomicBool::new(false));
    let second_call_tail = Arc::new(Mutex::new(None));
    let client = Arc::new(SteeredAfterToolUseAsyncApi {
        calls: AtomicUsize::new(0),
        steering: steering.clone(),
        first_call_ran_to_completion: Arc::clone(&first_call_ran_to_completion),
        second_call_tail: Arc::clone(&second_call_tail),
    });
    runtime.set_async_api_client(Arc::clone(&client) as Arc<dyn AsyncApiClient>);

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(collect_blocks(rx));
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime
        .run_turn_streaming("do it", tx, prompter)
        .await
        .expect("the turn should complete");
    drain.await.expect("drain");

    assert!(
        first_call_ran_to_completion.load(Ordering::SeqCst),
        "a call that already emitted a tool_use must not be abandoned"
    );
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        2,
        "one tool iteration plus the boundary-folded continuation"
    );
    let tail = second_call_tail
        .lock()
        .expect("tail slot")
        .clone()
        .expect("the second call recorded its tail");
    assert_eq!(
        tail.role,
        MessageRole::Tool,
        "the steer should ride the tool-result message, not a new user turn"
    );
    assert!(
        message_text(&tail).contains("stop after this"),
        "the tool-result boundary should carry the steer, got {:?}",
        message_text(&tail)
    );
    assert_wire_valid(&runtime.session().messages, "post-turn session");

    // A steer DID arrive here — it just arrived too late to abandon anything.
    // The firing must follow the abandonment, not the steer, or the ledger
    // would report the old boundary fold as if it were the new feature.
    assert_eq!(
        reissue_firings(),
        firings_before,
        "a steer delivered by the boundary fold must attest no re-issue firing"
    );
}
