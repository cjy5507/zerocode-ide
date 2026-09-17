//! L7c-1 — `AsyncApiClient` seam test.
//!
//! Verifies that when an [`AsyncApiClient`] is installed on a
//! [`ConversationRuntime`] via `set_async_api_client`, the
//! `run_turn_streaming` loop drives the async client instead of the
//! synchronous `ApiClient::stream`, propagates render deltas emitted
//! by the async client through `render_tx`, and still assembles the
//! assistant message from the returned [`AssistantEvent`]s for the
//! session bookkeeping path.
//!
//! This guards the Option (A) seam decision documented in
//! `.zo/tasks/L7c-tui-integration.md`: the runtime must support a
//! live SSE provider stack without disturbing the legacy sync path
//! used by the `-p` / ndjson CLI path.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use runtime::message_stream::types::BlockId;
use runtime::message_stream::{RenderBlock, SystemLevel};
use runtime::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError, PermissionPrompter,
    PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::Session;
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ConversationRuntime, PermissionMode,
    PermissionPolicy, RuntimeError, StaticToolExecutor, DEFAULT_STREAMING_CHANNEL_CAPACITY,
};
use tokio::sync::mpsc;

/// One process-global lock every test in this binary holds, so the quota tests
/// that set `ZO_SHARED_RATE_COORD` never race a test that reads it. Serializing
/// on it is required because the env var is process-global and `api` is compiled
/// without `cfg(test)`, so an ordinary run would otherwise read the developer's
/// live shared-quota observation.
static RATE_COORD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Whether the request's newest message is the persisted empty-response repair
/// reminder. Reminders are persisted as trailing `System` transcript messages
/// (not wire-only riders), so "the retry carries the reminder" means the
/// request tail is that System message; earlier persisted copies stay in
/// history by design and do not count as active.
fn last_message_is_repair_reminder(request: &ApiRequest) -> bool {
    request.messages.last().is_some_and(|message| {
        message.role == runtime::session::MessageRole::System
            && message.blocks.iter().any(|block| {
                matches!(
                    block,
                    runtime::ContentBlock::Text { text }
                        if text.contains("[zo:empty-response-retry]")
                )
            })
    })
}

/// User-prompt count in a request (persisted reminder/System annotations and
/// tool results excluded) — the "no duplicated user turn" invariant the empty
/// retry tests pin.
fn user_prompt_count(request: &ApiRequest) -> usize {
    request
        .messages
        .iter()
        .filter(|message| message.role == runtime::session::MessageRole::User)
        .count()
}

/// Panic-safe RAII guard held for the length of every test: it takes
/// [`RATE_COORD_LOCK`], snapshots the prior `ZO_SHARED_RATE_COORD`, and forces it
/// to `0` (shared coordination off → deterministic fallback), restoring the
/// snapshot on drop even if the test panics. Every test acquires it with one line
/// (`let _env = hermetic_env();`) so the whole binary is hermetic.
struct HermeticEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    prior: Option<std::ffi::OsString>,
}

impl Drop for HermeticEnv {
    fn drop(&mut self) {
        match &self.prior {
            Some(value) => std::env::set_var("ZO_SHARED_RATE_COORD", value),
            None => std::env::remove_var("ZO_SHARED_RATE_COORD"),
        }
    }
}

fn hermetic_env() -> HermeticEnv {
    let lock = RATE_COORD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prior = std::env::var_os("ZO_SHARED_RATE_COORD");
    std::env::set_var("ZO_SHARED_RATE_COORD", "0");
    HermeticEnv { _lock: lock, prior }
}

/// Sync client that should *never* be called when the async seam is
/// installed — its `stream` implementation panics to make any
/// accidental fallback loud.
struct ExplodingSyncApi;

impl ApiClient for ExplodingSyncApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        panic!("sync ApiClient::stream must not be called when async seam is installed");
    }
}

/// Async client that pushes a bespoke `System` render block (to prove
/// the runtime actually awaited our `stream_async`) plus a text-delta
/// at the iteration's reserved text block id, and returns a scripted
/// `AssistantEvent` sequence so the bookkeeping path still works.
struct ScriptedAsyncApi {
    calls: AtomicUsize,
}

impl ScriptedAsyncApi {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl AsyncApiClient for ScriptedAsyncApi {
    fn stream_async<'a>(
        &'a self,
        _request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);

            // Sentinel system block — proves the async path ran.
            render_tx
                .send(RenderBlock::System {
                    id: text_block_id,
                    level: SystemLevel::Info,
                    text: format!("async-seam-call-{call}"),
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;

            // Real text-delta into the reserved block id.
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: "hello from async".to_string(),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;

            Ok(vec![
                AssistantEvent::TextDelta("hello from async".to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

struct EmptyThenTextAsyncApi {
    calls: AtomicUsize,
}

impl EmptyThenTextAsyncApi {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl AsyncApiClient for EmptyThenTextAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let has_repair_reminder = last_message_is_repair_reminder(&request);

            match call {
                0 => return Ok(Vec::new()),
                1 => assert!(
                    has_repair_reminder,
                    "empty retry should carry the repair reminder"
                ),
                _ => assert!(
                    !has_repair_reminder,
                    "repair reminder should be cleared after a successful turn"
                ),
            }

            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: format!("recovered after empty {call}"),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;

            Ok(vec![
                AssistantEvent::TextDelta(format!("recovered after empty {call}")),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

struct ExhaustedEmptyThenTextAsyncApi {
    calls: AtomicUsize,
}

impl ExhaustedEmptyThenTextAsyncApi {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl AsyncApiClient for ExhaustedEmptyThenTextAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let has_repair_reminder = last_message_is_repair_reminder(&request);

            match call {
                0 => {
                    assert!(
                        !has_repair_reminder,
                        "first attempt should not start with a stale repair reminder"
                    );
                    assert_eq!(
                        request.messages.len(),
                        1,
                        "first empty turn should contain only the active user message"
                    );
                    return Ok(Vec::new());
                }
                1 | 2 => {
                    assert!(
                        has_repair_reminder,
                        "empty retries should carry the repair reminder"
                    );
                    assert_eq!(
                        user_prompt_count(&request),
                        1,
                        "empty retries should not accumulate extra user messages"
                    );
                    return Ok(Vec::new());
                }
                3 => {
                    assert!(
                        !has_repair_reminder,
                        "repair reminder should be cleared after the exhausted empty turn"
                    );
                    assert_eq!(
                        user_prompt_count(&request),
                        1,
                        "exhausted empty turn should roll back its orphan user message"
                    );
                }
                _ => {}
            }

            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: "recovered after exhausted empty".to_string(),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;

            Ok(vec![
                AssistantEvent::TextDelta("recovered after exhausted empty".to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

/// Models a text-only turn the user steers mid-stream. On call 0 it pushes a
/// steering message into the runtime's queue (simulating the TUI command pump
/// receiving an `AgentCommand::Steer` while the turn streams) and returns a
/// prose-only response — no tool calls. The runtime must therefore fold the
/// pending steer into a fresh user turn and loop once more rather than ending,
/// so call 1 sees the steering text as the latest user message.
struct SteeredTextOnlyAsyncApi {
    calls: AtomicUsize,
    steering: runtime::SteeringQueue,
}

impl SteeredTextOnlyAsyncApi {
    fn new(steering: runtime::SteeringQueue) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            steering,
        }
    }
}

impl AsyncApiClient for SteeredTextOnlyAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                // Simulate the user typing a course correction mid-turn.
                self.steering
                    .lock()
                    .expect("steering lock")
                    .push("use ripgrep instead".to_string());
            } else {
                // The folded steering must have arrived as the latest user turn.
                let last = request.messages.last().expect("a message");
                let joined: String = last
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        runtime::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    joined.contains("use ripgrep instead"),
                    "follow-up turn should carry the steering text, got {joined:?}"
                );
            }

            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: format!("text-only reply {call}"),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;

            Ok(vec![
                AssistantEvent::TextDelta(format!("text-only reply {call}")),
                AssistantEvent::MessageStop,
            ])
        })
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

#[tokio::test]
async fn async_api_client_seam_drives_run_turn_streaming() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );

    let async_client: Arc<dyn AsyncApiClient> = Arc::new(ScriptedAsyncApi::new());
    runtime.set_async_api_client(async_client);

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut blocks = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("streaming turn via async seam");
    let blocks = drain.await.expect("drain");

    // Loop should have completed in one iteration (no tool calls).
    assert_eq!(summary.iterations, 1);
    assert_eq!(summary.assistant_messages.len(), 1);
    assert!(summary.tool_results.is_empty());

    // Sentinel System block proves the async seam path ran — the
    // legacy sync-replay path never emits a System block for a
    // text-only turn.
    let saw_sentinel = blocks.iter().any(|block| {
        matches!(
            block,
            RenderBlock::System { text, .. } if text == "async-seam-call-0"
        )
    });
    assert!(saw_sentinel, "expected async-seam sentinel, got {blocks:?}");

    // The async client's text delta made it through.
    let saw_text = blocks.iter().any(|block| {
        matches!(
            block,
            RenderBlock::TextDelta { text, done, .. }
                if text == "hello from async" && *done
        )
    });
    assert!(saw_text, "expected async text delta, got {blocks:?}");
}

#[tokio::test]
async fn async_empty_stream_retries_instead_of_failing_missing_message_stop() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );

    let async_client: Arc<dyn AsyncApiClient> = Arc::new(EmptyThenTextAsyncApi::new());
    runtime.set_async_api_client(async_client);

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut blocks = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("empty async stream should retry and recover");
    let blocks = drain.await.expect("drain");

    assert_eq!(summary.iterations, 2);
    assert_eq!(summary.assistant_messages.len(), 1);
    assert!(blocks.iter().any(|block| {
        matches!(
            block,
            RenderBlock::TextDelta { text, done, .. }
                if text == "recovered after empty 1" && *done
        )
    }));

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut blocks = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("again", tx, prompter)
        .await
        .expect("later turns should not keep the empty-response reminder");
    let blocks = drain.await.expect("drain");

    assert_eq!(summary.iterations, 1);
    assert!(blocks.iter().any(|block| {
        matches!(
            block,
            RenderBlock::TextDelta { text, done, .. }
                if text == "recovered after empty 2" && *done
        )
    }));
}

#[tokio::test]
async fn async_repeated_empty_stream_ends_gracefully_without_runtime_error() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );

    let async_client: Arc<dyn AsyncApiClient> = Arc::new(ExhaustedEmptyThenTextAsyncApi::new());
    runtime.set_async_api_client(async_client);

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut blocks = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("repeated empty async streams should end the turn gracefully");
    let blocks = drain.await.expect("drain");

    // After the two bounded empty retries exhaust, the recovery cycle gives the
    // model one more attempt (call 3), which returns text — so the turn recovers
    // with content instead of synthesizing the "no assistant content" fallback.
    assert_eq!(summary.iterations, 4);
    assert_eq!(summary.assistant_messages.len(), 1);
    assert!(summary.tool_results.is_empty());
    assert!(blocks.iter().any(|block| {
        matches!(
            block,
            RenderBlock::TextDelta { text, done, .. }
                if text == "recovered after exhausted empty" && *done
        )
    }));

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut blocks = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("again", tx, prompter)
        .await
        .expect("later turn should run without a stale empty-response reminder");
    let blocks = drain.await.expect("drain");

    assert_eq!(summary.iterations, 1);
    assert_eq!(summary.assistant_messages.len(), 1);
    assert!(blocks.iter().any(|block| {
        matches!(
            block,
            RenderBlock::TextDelta { text, done, .. }
                if text == "recovered after exhausted empty" && *done
        )
    }));
}

#[tokio::test]
async fn text_only_turn_folds_pending_steering_into_followup_turn() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );

    // Share the runtime's steering queue with the scripted client so it can
    // enqueue a steer mid-turn, exactly as the TUI command pump does.
    let steering = runtime.steering_handle();
    let async_client: Arc<dyn AsyncApiClient> =
        Arc::new(SteeredTextOnlyAsyncApi::new(steering.clone()));
    runtime.set_async_api_client(async_client);

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut blocks = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("steered text-only turn should complete");
    let blocks = drain.await.expect("drain");

    // The turn must NOT have ended on the first text-only reply: a steer was
    // pending, so the loop folds it into a follow-up user turn and runs again.
    assert_eq!(
        summary.iterations, 2,
        "pending steering on a text-only turn should drive one more iteration"
    );
    // The queue is drained (no leftover steering stranded for the next turn).
    assert!(
        steering.lock().expect("steering lock").is_empty(),
        "steering queue should be drained after the turn"
    );
    // The user saw a `⤷ steering:` echo for the folded message.
    let saw_echo = blocks.iter().any(|block| {
        matches!(
            block,
            RenderBlock::System { text, .. } if text.contains("⤷ steering: use ripgrep instead")
        )
    });
    assert!(saw_echo, "expected steering echo, got {blocks:?}");
    // The post-steer reply streamed.
    assert!(blocks.iter().any(|block| matches!(
        block,
        RenderBlock::TextDelta { text, .. } if text == "text-only reply 1"
    )));
}

/// Async client whose first turn ends text-only at the output-token limit
/// (`stop_reason = "max_tokens"`, no tool call), then completes on the second.
/// Exercises the truncation-continuation on the *async streaming* path (the
/// production TUI / deep-gate path), which folds the continuation through the
/// same text-only-boundary merge as steering.
struct TruncatedThenDoneAsyncApi {
    calls: AtomicUsize,
}

impl AsyncApiClient for TruncatedThenDoneAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                // First turn: a plan preamble, then cut off at the output limit.
                render_tx
                    .send(RenderBlock::TextDelta {
                        id: text_block_id,
                        text: "I'll start by writing the file".to_string(),
                        done: true,
                    })
                    .await
                    .map_err(|_| RuntimeError::new("channel closed"))?;
                Ok(vec![
                    AssistantEvent::TextDelta("I'll start by writing the file".to_string()),
                    AssistantEvent::StopReason("max_tokens".to_string()),
                    AssistantEvent::MessageStop,
                ])
            } else {
                // The continuation nudge must have arrived as the latest user turn.
                let last = request.messages.last().expect("a message");
                let joined: String = last
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        runtime::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    joined.contains("[zo:truncation-continuation]"),
                    "continuation turn should carry the truncation nudge, got {joined:?}"
                );
                render_tx
                    .send(RenderBlock::TextDelta {
                        id: text_block_id,
                        text: "done".to_string(),
                        done: true,
                    })
                    .await
                    .map_err(|_| RuntimeError::new("channel closed"))?;
                Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::StopReason("end_turn".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        })
    }
}

#[tokio::test]
async fn async_truncated_text_only_turn_is_continued_not_ended() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    let async_client: Arc<dyn AsyncApiClient> = Arc::new(TruncatedThenDoneAsyncApi {
        calls: AtomicUsize::new(0),
    });
    runtime.set_async_api_client(async_client);

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut blocks = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("build the thing", tx, prompter)
        .await
        .expect("truncated turn should continue, not fail");
    let blocks = drain.await.expect("drain");

    // The output-limit truncation must NOT end the turn: the loop continues and
    // the second (completing) turn runs.
    assert_eq!(
        summary.iterations, 2,
        "an output-limit truncation on a text-only turn should drive one more iteration"
    );
    // The completing reply streamed.
    assert!(blocks.iter().any(|block| matches!(
        block,
        RenderBlock::TextDelta { text, .. } if text == "done"
    )));
}

// --- Refusal → latest Opus model fallback (Anthropic client-side fallback guidance) ---

const REFUSED_PARTIAL: &str = "REFUSED-PARTIAL-must-not-persist";

/// Records the `model_override` on every request, streams a refused partial on
/// the first call (`stop_reason: "refusal"`), and a clean answer on later calls.
/// The refused partial lets a test prove it is dropped from history.
struct RefusalThenAnswerAsyncApi {
    seen_overrides: std::sync::Mutex<Vec<Option<String>>>,
}

impl AsyncApiClient for RefusalThenAnswerAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = {
                let mut seen = self.seen_overrides.lock().expect("lock");
                seen.push(request.model_override.clone());
                seen.len() - 1
            };
            if call == 0 {
                // Stream a partial, then a refusal stop reason: the runtime must
                // drop this partial and retry on the fallback model.
                render_tx
                    .send(RenderBlock::TextDelta {
                        id: text_block_id,
                        text: REFUSED_PARTIAL.to_string(),
                        done: false,
                    })
                    .await
                    .map_err(|_| RuntimeError::new("channel closed"))?;
                Ok(vec![
                    AssistantEvent::TextDelta(REFUSED_PARTIAL.to_string()),
                    AssistantEvent::StopReason("refusal".to_string()),
                    AssistantEvent::MessageStop,
                ])
            } else {
                render_tx
                    .send(RenderBlock::TextDelta {
                        id: text_block_id,
                        text: "opus answer".to_string(),
                        done: true,
                    })
                    .await
                    .map_err(|_| RuntimeError::new("channel closed"))?;
                Ok(vec![
                    AssistantEvent::TextDelta("opus answer".to_string()),
                    AssistantEvent::StopReason("end_turn".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        })
    }
}

/// Refuses on every call regardless of the model, recording each override.
struct AlwaysRefuseAsyncApi {
    seen_overrides: std::sync::Mutex<Vec<Option<String>>>,
}

impl AsyncApiClient for AlwaysRefuseAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.seen_overrides
                .lock()
                .expect("lock")
                .push(request.model_override.clone());
            Ok(vec![
                AssistantEvent::StopReason("refusal".to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

fn drain_task(
    mut rx: mpsc::Receiver<RenderBlock>,
) -> tokio::task::JoinHandle<Vec<RenderBlock>> {
    tokio::spawn(async move {
        let mut blocks = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    })
}

fn history_text(runtime: &ConversationRuntime<ExplodingSyncApi, StaticToolExecutor>) -> String {
    runtime
        .session()
        .messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            runtime::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn refusal_on_fable_falls_back_to_opus_and_retries_once() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-fable-5");
    let client = Arc::new(RefusalThenAnswerAsyncApi {
        seen_overrides: std::sync::Mutex::new(Vec::new()),
    });
    runtime.set_async_api_client(client.clone());

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("fable refusal should fall back to opus and complete");
    let blocks = drain.await.expect("drain");

    // Exactly two model calls: the refused Fable turn, then the Opus retry.
    let seen = client.seen_overrides.lock().expect("lock").clone();
    assert_eq!(
        seen,
        vec![None, Some(api::latest_anthropic_model().to_string())],
        "the retry must carry the latest Opus model model override; got {seen:?}"
    );
    assert_eq!(summary.iterations, 2);

    // A System warn line announced the fallback.
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { level: SystemLevel::Warn, text, .. }
                if text.contains("latest Opus model")
        )),
        "expected an latest Opus model fallback warn, got {blocks:?}"
    );

    // The refused partial is NOT in history; the Opus answer is.
    let history = history_text(&runtime);
    assert!(
        !history.contains(REFUSED_PARTIAL),
        "refused partial must not persist in history, got {history:?}"
    );
    assert!(
        history.contains("opus answer"),
        "the fallback model's answer must be recorded, got {history:?}"
    );
}

#[tokio::test]
async fn refusal_after_fallback_is_surfaced_not_looped() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-fable-5");
    let client = Arc::new(AlwaysRefuseAsyncApi {
        seen_overrides: std::sync::Mutex::new(Vec::new()),
    });
    runtime.set_async_api_client(client.clone());

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("a doubly-refused turn should end with a notice, not error");
    let blocks = drain.await.expect("drain");

    // Fable refused, we fell back to Opus, Opus refused too — exactly two calls,
    // then the fallback is capped (no infinite loop).
    let seen = client.seen_overrides.lock().expect("lock").clone();
    assert_eq!(
        seen,
        vec![None, Some(api::latest_anthropic_model().to_string())],
        "the fallback must fire once; a second refusal must not loop, got {seen:?}"
    );
    assert_eq!(summary.iterations, 2);
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { level: SystemLevel::Warn, text, .. }
                if text.contains("declined")
        )),
        "expected a surfaced refusal notice, got {blocks:?}"
    );
    // The turn is well-formed: it ended with a recorded assistant notice.
    assert!(!summary.assistant_messages.is_empty());
}

#[tokio::test]
async fn refusal_on_opus_retries_the_same_model_once_then_surfaces() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    // Already on Opus: one retry on the same model (no override), then the
    // refusal is surfaced honestly rather than looping or swapping models.
    runtime.set_context_model(api::latest_anthropic_model());
    let client = Arc::new(AlwaysRefuseAsyncApi {
        seen_overrides: std::sync::Mutex::new(Vec::new()),
    });
    runtime.set_async_api_client(client.clone());

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("an opus refusal should surface, not error");
    let _ = drain.await.expect("drain");

    let seen = client.seen_overrides.lock().expect("lock").clone();
    assert_eq!(
        seen,
        vec![None, None],
        "the retry must stay on the same model — no fallback override, got {seen:?}"
    );
    assert_eq!(summary.iterations, 2);

    // The retry budget is per PUBLIC turn, not per session: a second turn on
    // the same runtime gets its own retry (this pinned a real bug — the
    // streaming entry point forgot the reset, so a session spent its one
    // retry on the first refusal ever and surfaced every later one).
    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi again", tx, prompter)
        .await
        .expect("the second turn's refusal should also surface, not error");
    let _ = drain.await.expect("drain");
    let seen = client.seen_overrides.lock().expect("lock").clone();
    assert_eq!(
        seen.len(),
        4,
        "turn two must retry once more (2 calls per turn), got {seen:?}"
    );
    assert_eq!(summary.iterations, 2);
}

#[tokio::test]
async fn non_refusal_stop_reason_on_fable_is_unaffected() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-fable-5");
    // Reuse RefusalThenAnswer but never reach call 0's refusal path: a clean
    // end_turn on the first call proves the refusal gate does not misfire.
    let client = Arc::new(ScriptedAsyncApi::new());
    runtime.set_async_api_client(client);

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("a non-refusal fable turn must complete in one pass");
    let blocks = drain.await.expect("drain");

    assert_eq!(summary.iterations, 1);
    assert!(
        !blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { text, .. } if text.contains("latest Opus model") || text.contains("declined")
        )),
        "a non-refusal turn must emit no refusal notice, got {blocks:?}"
    );
    assert!(history_text(&runtime).contains("hello from async"));
}

// --- Quota exhaustion → cross-provider fallback (P3) --------------------------

use runtime::ProviderErrorClass;
use std::time::Duration;

/// Async client whose leading `fail_until` calls fail with a `RateLimit`
/// (quota-exhausted) error and answer cleanly afterward. The error message is
/// deliberately keyword-free so the retry classifier fails it immediately
/// (attempt 0) — the fallback swap keys off the stored `RateLimit` *class*, not
/// the text — keeping the test fast without touching the retry budget.
struct RateLimitingAsyncApi {
    calls: AtomicUsize,
    fail_until: usize,
    retry_after: Option<Duration>,
    answer: String,
}

impl RateLimitingAsyncApi {
    fn new(fail_until: usize, retry_after: Option<Duration>, answer: &str) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            fail_until,
            retry_after,
            answer: answer.to_string(),
        }
    }
}

impl AsyncApiClient for RateLimitingAsyncApi {
    fn stream_async<'a>(
        &'a self,
        _request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call < self.fail_until {
                return Err(RuntimeError::with_provider_error_class(
                    "simulated quota exhaustion (test)",
                    ProviderErrorClass::account_rate_limit(self.retry_after),
                ));
            }
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: self.answer.clone(),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            Ok(vec![
                AssistantEvent::TextDelta(self.answer.clone()),
                AssistantEvent::StopReason("end_turn".to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

/// Fallback async client that records the calls it received and each request's
/// `model_override`, then streams a clean answer. Used to prove the fallback
/// carried the turn and never received a stale (refusal→Opus) override.
struct RecordingAnswerAsyncApi {
    calls: AtomicUsize,
    seen_overrides: std::sync::Mutex<Vec<Option<String>>>,
    answer: String,
}

impl RecordingAnswerAsyncApi {
    fn new(answer: &str) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            seen_overrides: std::sync::Mutex::new(Vec::new()),
            answer: answer.to_string(),
        }
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AsyncApiClient for RecordingAnswerAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.seen_overrides
                .lock()
                .expect("lock")
                .push(request.model_override.clone());
            self.calls.fetch_add(1, Ordering::SeqCst);
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: self.answer.clone(),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            Ok(vec![
                AssistantEvent::TextDelta(self.answer.clone()),
                AssistantEvent::StopReason("end_turn".to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

fn quota_runtime(
    main: Arc<dyn AsyncApiClient>,
) -> ConversationRuntime<ExplodingSyncApi, StaticToolExecutor> {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-fable-5");
    runtime.set_async_api_client(main);
    runtime
}

/// (1) A hard main-model quota exhaustion swaps this turn onto the
/// cross-provider fallback, announces it, and completes on the fallback model.
#[tokio::test]
async fn quota_exhaustion_swaps_to_fallback_and_completes_the_turn() {
    let _env = hermetic_env();
    let main = Arc::new(RateLimitingAsyncApi::new(1, None, "main answer"));
    let mut runtime = quota_runtime(main.clone());
    let fallback = Arc::new(RecordingAnswerAsyncApi::new("fallback answer"));
    runtime.set_quota_fallback_client(Some((fallback.clone(), "gpt-peer-x".to_string())));

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("a quota-exhausted turn should continue on the fallback, not fail");
    let blocks = drain.await.expect("drain");

    assert_eq!(main.calls.load(Ordering::SeqCst), 1, "main model hit exactly once (the exhaustion)");
    assert_eq!(fallback.call_count(), 1, "the fallback carried the turn");
    assert_eq!(summary.iterations, 2);
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { level: SystemLevel::Warn, text, .. }
                if text.contains("quota") && text.contains("gpt-peer-x")
        )),
        "expected a quota-swap warn naming the fallback, got {blocks:?}"
    );
    assert!(history_text(&runtime).contains("fallback answer"));
}

/// (2) The next turn pre-arms straight onto the fallback from the session
/// cooldown — the main model's retry budget is NOT re-spent (it is never hit).
#[tokio::test]
async fn next_turn_prearms_onto_fallback_without_reburning_main() {
    let _env = hermetic_env();
    let main = Arc::new(RateLimitingAsyncApi::new(1, None, "main answer"));
    let mut runtime = quota_runtime(main.clone());
    let fallback = Arc::new(RecordingAnswerAsyncApi::new("fallback answer"));
    runtime.set_quota_fallback_client(Some((fallback.clone(), "gpt-peer-x".to_string())));

    // Turn 1 exhausts the main model and records the (15-min default) cooldown.
    let (tx1, rx1) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain1 = drain_task(rx1);
    let p1: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime.run_turn_streaming("turn one", tx1, p1).await.expect("turn 1 completes");
    let _ = drain1.await;
    assert_eq!(main.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fallback.call_count(), 1);

    // Turn 2 must pre-arm: the main client is not hit again, and a pre-arm info
    // line announces the continuation.
    let (tx2, rx2) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain2 = drain_task(rx2);
    let p2: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime.run_turn_streaming("turn two", tx2, p2).await.expect("turn 2 completes on fallback");
    let blocks2 = drain2.await.expect("drain");

    assert_eq!(main.calls.load(Ordering::SeqCst), 1, "the main model's retry budget must NOT be re-spent");
    assert_eq!(fallback.call_count(), 2, "turn 2 runs on the fallback from the first request");
    assert!(
        blocks2.iter().any(|block| matches!(
            block,
            RenderBlock::System { level: SystemLevel::Info, text, .. }
                if text.contains("cooling down") && text.contains("gpt-peer-x")
        )),
        "expected a pre-arm info line, got {blocks2:?}"
    );
}

/// (3) Once the cooldown elapses the session returns to the main model on its
/// own — no fallback is used on the recovered turn.
#[tokio::test]
async fn main_model_recovers_after_cooldown_elapses() {
    let _env = hermetic_env();
    // A tiny retry_after so the cooldown lapses between the two turns.
    let main = Arc::new(RateLimitingAsyncApi::new(1, Some(Duration::from_millis(1)), "main answer"));
    let mut runtime = quota_runtime(main.clone());
    let fallback = Arc::new(RecordingAnswerAsyncApi::new("fallback answer"));
    runtime.set_quota_fallback_client(Some((fallback.clone(), "gpt-peer-x".to_string())));

    let (tx1, rx1) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain1 = drain_task(rx1);
    let p1: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime.run_turn_streaming("turn one", tx1, p1).await.expect("turn 1 completes on fallback");
    let _ = drain1.await;
    assert_eq!(fallback.call_count(), 1);

    // Let the 1ms cooldown pass.
    tokio::time::sleep(Duration::from_millis(25)).await;

    let (tx2, rx2) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain2 = drain_task(rx2);
    let p2: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("turn two", tx2, p2)
        .await
        .expect("turn 2 recovers on the main model");
    let _ = drain2.await;

    assert_eq!(main.calls.load(Ordering::SeqCst), 2, "turn 2 hits the recovered main model");
    assert_eq!(fallback.call_count(), 1, "the fallback is NOT used once the cooldown clears");
    assert_eq!(summary.iterations, 1, "the recovered turn completes in one pass");
    assert!(history_text(&runtime).contains("main answer"));
}

/// (4) A fallback that is itself rate-limited ends the turn — no second
/// fallback (the one-shot cap).
#[tokio::test]
async fn fallback_that_is_also_rate_limited_fails_the_turn() {
    let _env = hermetic_env();
    let main = Arc::new(RateLimitingAsyncApi::new(1, None, "main answer"));
    let mut runtime = quota_runtime(main.clone());
    // The fallback always rate-limits too.
    let fallback = Arc::new(RateLimitingAsyncApi::new(usize::MAX, None, "never"));
    runtime.set_quota_fallback_client(Some((fallback.clone(), "gpt-peer-x".to_string())));

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let result = runtime.run_turn_streaming("hi", tx, prompter).await;
    let _ = drain.await;

    assert!(result.is_err(), "a fallback that also 429s must fail the turn, not loop");
    assert_eq!(main.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fallback.calls.load(Ordering::SeqCst), 1, "the fallback is tried exactly once (no second fallback)");
}

/// (5) With no fallback client installed (feature off), a quota-exhausted turn
/// fails exactly as before — no swap.
#[tokio::test]
async fn quota_fallback_off_preserves_the_failing_behavior() {
    let _env = hermetic_env();
    let main = Arc::new(RateLimitingAsyncApi::new(1, None, "main answer"));
    let mut runtime = quota_runtime(main.clone());
    // No set_quota_fallback_client → feature effectively off.

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let result = runtime.run_turn_streaming("hi", tx, prompter).await;
    let _ = drain.await;

    assert!(result.is_err(), "with the feature off a quota-exhausted turn must fail");
    assert_eq!(main.calls.load(Ordering::SeqCst), 1);
}

/// (6) Refusal + quota fallback coexist: while running on a non-Anthropic
/// fallback, a `refusal` stop reason must NOT arm the Opus override (that would
/// force the current Opus catalog head onto the wrong provider). `effective_request_model`
/// judges the active (fallback) model, so the refusal path yields Proceed.
#[tokio::test]
async fn refusal_on_quota_fallback_does_not_arm_opus_override() {
    let _env = hermetic_env();
    let main = Arc::new(RateLimitingAsyncApi::new(1, None, "main answer"));
    let mut runtime = quota_runtime(main.clone());
    // The fallback (a non-Anthropic peer) refuses on its first call.
    let fallback = Arc::new(RefusalThenAnswerAsyncApi {
        seen_overrides: std::sync::Mutex::new(Vec::new()),
    });
    runtime.set_quota_fallback_client(Some((fallback.clone(), "gpt-peer-x".to_string())));

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let _ = runtime.run_turn_streaming("hi", tx, prompter).await;
    let _ = drain.await;

    let seen = fallback.seen_overrides.lock().expect("lock").clone();
    assert!(
        !seen.iter().any(|override_id| override_id.as_deref() == Some(api::latest_anthropic_model())),
        "a refusal on the non-Anthropic fallback must not inject the Opus override, got {seen:?}"
    );
}

/// Sync (`run_turn`) path parity: a quota-exhausted headless text turn swaps
/// onto the async cross-provider fallback via the sync→async bridge and
/// completes, without the sync main client being re-hit.
#[test]
fn sync_run_turn_swaps_to_quota_fallback() {
    struct RateLimitingSyncApi {
        calls: std::sync::atomic::AtomicUsize,
    }
    impl ApiClient for RateLimitingSyncApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(RuntimeError::with_provider_error_class(
                "simulated quota exhaustion (test)",
                ProviderErrorClass::account_rate_limit(None),
            ))
        }
    }

    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        RateLimitingSyncApi {
            calls: std::sync::atomic::AtomicUsize::new(0),
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-fable-5");
    let fallback = Arc::new(RecordingAnswerAsyncApi::new("sync fallback answer"));
    runtime.set_quota_fallback_client(Some((fallback.clone(), "gpt-peer-x".to_string())));

    let summary = runtime
        .run_turn("hi", None)
        .expect("the sync turn should continue on the async fallback, not fail");

    assert_eq!(fallback.call_count(), 1, "the async fallback carried the sync turn");
    assert!(summary.iterations >= 2, "the sync loop re-requested on the fallback");
    let history = runtime
        .session()
        .messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            runtime::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(history.contains("sync fallback answer"), "got {history:?}");
}
