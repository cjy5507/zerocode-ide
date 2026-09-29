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

// --- Refusal → the same model once, then the category's route (t-6747) ---

const REFUSED_PARTIAL: &str = "REFUSED-PARTIAL-must-not-persist";

/// Where the catalog routes a `cyber` decline on `model` on its own provider —
/// read from the catalog, never spelled here.
fn cyber_route(model: &str) -> String {
    let provider = api::detect_provider_kind(model);
    api::refusal_route_candidates(model, Some("cyber"))
        .into_iter()
        .find(|candidate| api::detect_provider_kind(candidate) == provider)
        .expect("the catalog routes a cyber decline on this lineup")
}

/// Where it routes the same decline across providers, first.
fn cyber_route_across(model: &str) -> String {
    let provider = api::detect_provider_kind(model);
    api::refusal_route_candidates(model, Some("cyber"))
        .into_iter()
        .find(|candidate| api::detect_provider_kind(candidate) != provider)
        .expect("the catalog routes a cyber decline across providers too")
}

/// Records the `model_override` on every request, streams a refused partial on
/// the first two calls (`stop_reason: "refusal"`, category `cyber`) — the
/// declined request and its same-model retry — and a clean answer after.
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
            if call < 2 {
                // Stream a partial, then a refusal stop reason: the runtime must
                // drop this partial and retry — the same model, then the route.
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
                    AssistantEvent::RefusalCategory("cyber".to_string()),
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

/// Refuses on every call regardless of the model, recording each override —
/// in `category`, when the test names one.
struct AlwaysRefuseAsyncApi {
    seen_overrides: std::sync::Mutex<Vec<Option<String>>>,
    category: Option<&'static str>,
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
            let mut events = vec![AssistantEvent::StopReason("refusal".to_string())];
            runtime::push_refusal_category(&mut events, self.category);
            events.push(AssistantEvent::MessageStop);
            Ok(events)
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
async fn refusal_on_fable_retries_once_then_continues_on_the_categorys_route() {
    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-fable-5");
    // The route without a question (t-7153): nobody is at this keyboard.
    runtime.set_classifier_fallback(runtime::ClassifierFallback::Auto);
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
        .expect("fable refusal should continue on the route and complete");
    let blocks = drain.await.expect("drain");

    // Three calls: the declined Fable turn, its same-model retry, then the
    // category's route.
    let route = cyber_route("claude-fable-5");
    let seen = client.seen_overrides.lock().expect("lock").clone();
    assert_eq!(
        seen,
        vec![None, None, Some(route.clone())],
        "the same model once, then the route's override; got {seen:?}"
    );
    assert_eq!(summary.iterations, 3);

    // The receipt names the model it left and the one it continues on (the
    // notice carries the catalog's route, not a hardcoded name).
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { level: SystemLevel::Warn, text, .. }
                if text.contains(&route) && text.contains("instead of claude-fable-5")
        )),
        "expected a route receipt naming {route}, got {blocks:?}"
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
    // The route without a question (t-7153): nobody is at this keyboard.
    runtime.set_classifier_fallback(runtime::ClassifierFallback::Auto);
    let client = Arc::new(AlwaysRefuseAsyncApi {
        seen_overrides: std::sync::Mutex::new(Vec::new()),
        category: Some("cyber"),
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

    // Fable refused, the same model refused again, the route refused too —
    // exactly three calls, then the ladder is spent (no infinite loop).
    let seen = client.seen_overrides.lock().expect("lock").clone();
    assert_eq!(
        seen,
        vec![None, None, Some(cyber_route("claude-fable-5"))],
        "each rung fires once; a refusal on the route must not loop, got {seen:?}"
    );
    assert_eq!(summary.iterations, 3);
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
    // The route without a question (t-7153): nobody is at this keyboard.
    runtime.set_classifier_fallback(runtime::ClassifierFallback::Auto);
    // A refusal that names no category: it stands after the one retry.
    let client = Arc::new(AlwaysRefuseAsyncApi {
        seen_overrides: std::sync::Mutex::new(Vec::new()),
        category: None,
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

    // The decline came back for the same conversation in the same category
    // (none) after every step of the ladder was spent on it last turn
    // (t-15890): turn two asks once, with the person's words as written, and
    // says so — the same-model retry is not asked a second time. (A refusal
    // naming no category takes no context-cleaning retry either, t-6747: only
    // a category the provider routes is walked past its first answer.)
    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi again", tx, prompter)
        .await
        .expect("the second turn's refusal should also surface, not error");
    let blocks = drain.await.expect("drain");
    let seen = client.seen_overrides.lock().expect("lock").clone();
    assert_eq!(
        seen.len(),
        3,
        "turn two: the declined request only; got {seen:?}"
    );
    assert_eq!(summary.iterations, 1);
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { text, .. } if text.contains("will be declined again")
        )),
        "and says what a bare continue will meet, got {blocks:?}"
    );

    // The retry budget is still per PUBLIC turn, not per session (this pinned
    // a real bug — the streaming entry point forgot the reset, so a session
    // spent its one retry on the first refusal ever and surfaced every later
    // one): once the conversation has changed since, a decline is a new
    // decline and walks the ladder from its first rung. A dozen exchanges of
    // work in between is far past what the ladder counts as the same
    // conversation.
    let earlier_work = (0..24).map(|n| {
        if n % 2 == 0 {
            runtime::ConversationMessage::user_text(format!("earlier work {n}"))
        } else {
            runtime::ConversationMessage::assistant(vec![runtime::ContentBlock::Text {
                text: format!("earlier answer {n}"),
            }])
        }
    });
    Arc::make_mut(&mut runtime.session_mut().messages).extend(earlier_work);
    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi once more", tx, prompter)
        .await
        .expect("the third turn's refusal should also surface, not error");
    let _ = drain.await.expect("drain");
    let seen = client.seen_overrides.lock().expect("lock").clone();
    // The ladder as it always ran: the request, its same-model retry, and — the
    // conversation being long enough to fold now — the compaction (its summary
    // is a call this client declines too) and the retry after it: four calls.
    assert_eq!(
        seen.len(),
        7,
        "turn three: the ladder from the first rung, four calls; got {seen:?}"
    );
    assert!(
        summary.iterations >= 2,
        "the request and its same-model retry ran again: {}",
        summary.iterations
    );
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
    // The route without a question (t-7153): nobody is at this keyboard.
    runtime.set_classifier_fallback(runtime::ClassifierFallback::Auto);
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

// --- Refusal → cross-provider handoff, pre-arm, context clean (t-6269) --------

/// An async client that refuses (`stop_reason: "refusal"`) whenever the request
/// still carries `poison` in any message, and answers cleanly once it is gone.
/// Drives both the "always refuses" main model (the poison rides its own input)
/// and the P3 context-clean recovery (dropping the earlier exchange removes it).
struct RefuseWhilePoisonAsyncApi {
    poison: String,
    answer: String,
    calls: AtomicUsize,
}

impl RefuseWhilePoisonAsyncApi {
    fn new(poison: &str, answer: &str) -> Self {
        Self {
            poison: poison.to_string(),
            answer: answer.to_string(),
            calls: AtomicUsize::new(0),
        }
    }
}

impl AsyncApiClient for RefuseWhilePoisonAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let poisoned = request.messages.iter().any(|message| {
                message.blocks.iter().any(|block| {
                    matches!(block, runtime::ContentBlock::Text { text } if text.contains(&self.poison))
                })
            });
            if poisoned {
                return Ok(vec![
                    AssistantEvent::StopReason("refusal".to_string()),
                    AssistantEvent::RefusalCategory("cyber".to_string()),
                    AssistantEvent::MessageStop,
                ]);
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

fn opus_runtime() -> ConversationRuntime<ExplodingSyncApi, StaticToolExecutor> {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    // Opus carries Fable's classifiers: its `cyber` ladder is the same model,
    // the category's route on its own provider, then the cross-provider
    // handoff (t-6747).
    runtime.set_context_model(api::latest_anthropic_model());
    // The route without a question (t-7153): nobody is at this keyboard.
    runtime.set_classifier_fallback(runtime::ClassifierFallback::Auto);
    runtime
}

/// Opus refuses, the same-model retry refuses, the category's route on its
/// own provider refuses too, and the turn is handed to the installed
/// cross-provider refusal client the category routes to — which answers. A
/// warn line names that peer, and the wire badge shows it as a refusal
/// fallback.
#[tokio::test]
async fn opus_refusal_hands_the_turn_to_the_cross_provider_client() {
    use runtime::message_stream::WireModelSource;

    let _env = hermetic_env();
    let mut runtime = opus_runtime();
    let main = Arc::new(AlwaysRefuseAsyncApi {
        seen_overrides: std::sync::Mutex::new(Vec::new()),
        category: Some("cyber"),
    });
    runtime.set_async_api_client(main.clone());
    let peer = Arc::new(RecordingAnswerAsyncApi::new("peer answer"));
    let across = cyber_route_across(api::latest_anthropic_model());
    runtime.set_refusal_fallback_client(Some((peer.clone(), across.clone())));

    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect("a doubly-refused Opus turn should continue on the peer, not fail");
    let blocks = drain.await.expect("drain");

    // The main client was asked three times (the declined request, its
    // same-model retry, the route), then the peer carried the turn once.
    assert_eq!(
        *main.seen_overrides.lock().expect("lock"),
        vec![None, None, Some(cyber_route(api::latest_anthropic_model()))]
    );
    assert_eq!(peer.call_count(), 1, "the cross-provider refusal client answered");
    assert_eq!(summary.iterations, 4);
    assert!(history_text(&runtime).contains("peer answer"));

    // A warn line named the peer, and the wire badge showed it as a refusal
    // fallback (not a quota fallback).
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { level: SystemLevel::Warn, text, .. }
                if text.contains(&across) && text.contains("another provider")
        )),
        "expected a cross-provider refusal warn naming the peer, got {blocks:?}"
    );
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::WireModel(wire)
                if wire.model == across && wire.source == WireModelSource::RefusalFallback
        )),
        "expected the peer announced as a refusal fallback on the wire, got {blocks:?}"
    );
}

/// After two consecutive refusal turns, the session pre-arms the category's
/// route: a following short turn never sends the refused Opus model its
/// request (the sticky classifier never sees it) — its first request rides the
/// route, and a pre-arm notice names it.
#[tokio::test]
async fn consecutive_refusal_turns_prearm_the_next_turn_on_the_categorys_route() {
    let _env = hermetic_env();
    let mut runtime = opus_runtime();
    let main = Arc::new(AlwaysRefuseAsyncApi {
        seen_overrides: std::sync::Mutex::new(Vec::new()),
        category: Some("cyber"),
    });
    runtime.set_async_api_client(main.clone());
    let peer = Arc::new(RecordingAnswerAsyncApi::new("peer answer"));
    let across = cyber_route_across(api::latest_anthropic_model());
    runtime.set_refusal_fallback_client(Some((peer.clone(), across)));

    for turn in ["turn one", "turn two"] {
        let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
        let drain = drain_task(rx);
        let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
        runtime.run_turn_streaming(turn, tx, prompter).await.expect("turn completes on peer");
        let _ = drain.await;
    }
    // Two refusal turns: the main client asked three times each (declined,
    // same-model retry, route), the peer once each.
    assert_eq!(main.seen_overrides.lock().expect("lock").len(), 6);
    assert_eq!(peer.call_count(), 2);

    // The follow-up pre-arms the route: its first request carries the route's
    // override — the refused Opus model is never sent it — and a pre-arm notice
    // names the route.
    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime.run_turn_streaming("why", tx, prompter).await.expect("follow-up completes on peer");
    let blocks = drain.await.expect("drain");

    let route = cyber_route(api::latest_anthropic_model());
    let seen = main.seen_overrides.lock().expect("lock").clone();
    assert_eq!(
        seen.get(6),
        Some(&Some(route.clone())),
        "the pre-armed follow-up's first request rides the route: {seen:?}"
    );
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { text, .. }
                if text.contains(&route) && text.contains("continuing this session")
        )),
        "expected a pre-arm notice naming the route, got {blocks:?}"
    );
}

/// With no provider fallback, a refusal whose earlier declined exchange is still
/// in context takes the P3 context-cleaning retry: dropping that exchange lets
/// the same model answer, and a notice says what was dropped.
#[tokio::test]
async fn context_cleaning_retry_recovers_when_the_earlier_exchange_is_dropped() {
    let _env = hermetic_env();
    let mut runtime = opus_runtime();
    // No refusal_fallback_client and no quota fallback: the only recovery left
    // is dropping the poisoned exchange. The client refuses while "POISON" is in
    // context and answers once it is gone.
    let client = Arc::new(RefuseWhilePoisonAsyncApi::new("POISON", "clean answer"));
    runtime.set_async_api_client(client.clone());

    // Turn one carries the poison and, with no fallback, surfaces a refusal —
    // leaving that declined exchange in history.
    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime.run_turn_streaming("please POISON this", tx, prompter).await.expect("turn one surfaces");
    let _ = drain.await;
    assert!(history_text(&runtime).contains("POISON"));

    // The follow-up refuses (poison still in context) through the same-model
    // retry and the route, then the context-cleaning retry drops the earlier
    // exchange and the clean re-ask answers.
    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime.run_turn_streaming("why not", tx, prompter).await.expect("follow-up recovers");
    let blocks = drain.await.expect("drain");

    assert!(history_text(&runtime).contains("clean answer"), "the clean re-ask must answer");
    assert!(
        !history_text(&runtime).contains("POISON"),
        "the poisoned exchange must be gone from history"
    );
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { text, .. } if text.contains("dropped from context")
        )),
        "expected a context-cleaning notice, got {blocks:?}"
    );
    assert_eq!(
        summary.iterations, 4,
        "initial refuse, same-model retry, the route, clean re-ask"
    );
}

/// The give-up path: no fallback and the context clean does not help either, so
/// the turn surfaces after exhausting the same-model retry and the one clean.
#[tokio::test]
async fn a_refusal_with_no_fallback_surfaces_after_the_context_clean() {
    let _env = hermetic_env();
    let mut runtime = opus_runtime();
    // Always refuses regardless of context — the clean cannot help.
    let client = Arc::new(AlwaysRefuseAsyncApi {
        seen_overrides: std::sync::Mutex::new(Vec::new()),
        category: Some("cyber"),
    });
    runtime.set_async_api_client(client.clone());

    // Turn one surfaces (creating a prior declined exchange).
    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime.run_turn_streaming("one", tx, prompter).await.expect("surfaces");
    let _ = drain.await;
    let after_one = client.seen_overrides.lock().expect("lock").len();

    // Turn two: the declined request, the same-model retry, the route, then
    // the context clean, then surface — four calls, ending in a surfaced
    // notice.
    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime.run_turn_streaming("two", tx, prompter).await.expect("surfaces, not error");
    let blocks = drain.await.expect("drain");

    assert_eq!(
        client.seen_overrides.lock().expect("lock").len() - after_one,
        4,
        "turn two: declined, same-model retry, route, context clean, then surface"
    );
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { text, .. } if text.contains("giving up on automatic")
        )),
        "expected a surfaced refusal notice mentioning /model, got {blocks:?}"
    );
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { text, .. } if text.contains("/model")
        )),
        "the surfaced notice must point at /model for another provider, got {blocks:?}"
    );
    assert!(!summary.assistant_messages.is_empty());
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

// --- A 5xx that outlives the retry ladder → one-tier demotion (t-15565) --------

/// The text the reported turn died on (2026-09-22), as the runtime sees it once the
/// `api` layer's five re-opens are spent: an SSE `api_error` frame, which rides an
/// HTTP 200 and so has no status in it.
const SPENT_API_ERROR_FRAME: &str =
    "provider stream: transport error: api stream error (api_error): Internal server error";

/// A provider whose SERVER is failing the heavy tier, as the runtime meets it after
/// the `api` layer has spent its ladder: a request that still names the session's own
/// model ends with `text` under `class`, and one that carries a lighter tier's
/// override is answered — or, with `lighter_fails`, ends the same way. Every call
/// records its override, and the two moments a latency reading needs.
struct ServerFaultingAsyncApi {
    text: &'static str,
    class: ProviderErrorClass,
    lighter_fails: bool,
    calls: AtomicUsize,
    seen_overrides: std::sync::Mutex<Vec<Option<String>>>,
    marks: std::sync::Mutex<Vec<(&'static str, std::time::Instant)>>,
    answer: String,
}

impl ServerFaultingAsyncApi {
    fn new(text: &'static str, class: ProviderErrorClass, answer: &str) -> Self {
        Self {
            text,
            class,
            lighter_fails: false,
            calls: AtomicUsize::new(0),
            seen_overrides: std::sync::Mutex::new(Vec::new()),
            marks: std::sync::Mutex::new(Vec::new()),
            answer: answer.to_string(),
        }
    }

    fn also_failing_on_the_lighter_tier(mut self) -> Self {
        self.lighter_fails = true;
        self
    }

    fn overrides(&self) -> Vec<Option<String>> {
        self.seen_overrides.lock().expect("lock").clone()
    }

    /// The instants the first `what` mark was taken at, if any.
    fn mark(&self, what: &str) -> Option<std::time::Instant> {
        self.marks
            .lock()
            .expect("lock")
            .iter()
            .find(|(name, _)| *name == what)
            .map(|(_, at)| *at)
    }
}

impl AsyncApiClient for ServerFaultingAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen_overrides
                .lock()
                .expect("lock")
                .push(request.model_override.clone());
            if request.model_override.is_none() || self.lighter_fails {
                self.marks
                    .lock()
                    .expect("lock")
                    .push(("failed", std::time::Instant::now()));
                return Err(RuntimeError::with_provider_error_class(self.text, self.class));
            }
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: self.answer.clone(),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            self.marks
                .lock()
                .expect("lock")
                .push(("first token", std::time::Instant::now()));
            Ok(vec![
                AssistantEvent::TextDelta(self.answer.clone()),
                AssistantEvent::StopReason("end_turn".to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

/// A runtime on a model that has a lighter rung on its own provider's ladder — the
/// demotion needs one.
fn demotable_runtime(
    main: Arc<dyn AsyncApiClient>,
) -> ConversationRuntime<ExplodingSyncApi, StaticToolExecutor> {
    let mut runtime = quota_runtime(main);
    runtime.set_context_model("claude-opus-5");
    runtime
}

/// One streaming turn: its iteration count (or the error's words) and every block it drew.
async fn run_turn_reporting(
    runtime: &mut ConversationRuntime<ExplodingSyncApi, StaticToolExecutor>,
    input: &str,
) -> (Result<usize, String>, Vec<RenderBlock>) {
    let (tx, rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = drain_task(rx);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let result = runtime
        .run_turn_streaming(input, tx, prompter)
        .await
        .map(|summary| summary.iterations)
        .map_err(|error| error.to_string());
    (result, drain.await.expect("drain"))
}

fn warn_texts(blocks: &[RenderBlock]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|block| match block {
            RenderBlock::System {
                level: SystemLevel::Warn,
                text,
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// (1) The reported turn: the heavy tier's ladder is spent on an `api_error` frame.
/// The turn continues one tier down instead of dying, and the NEXT turn starts on the
/// model the person chose again — with its own one demotion when its ladder is spent.
///
/// Prints the reading the slice asks for: how long the runtime takes from the spent
/// ladder to the lighter tier's first token. The fake answers at once, so it is the
/// runtime's own cost (the demotion notice and the rebuilt request), not a model's.
#[tokio::test]
async fn a_5xx_that_outlives_the_ladder_continues_the_turn_one_tier_down() {
    let _env = hermetic_env();
    let main = Arc::new(ServerFaultingAsyncApi::new(
        SPENT_API_ERROR_FRAME,
        ProviderErrorClass::Transient,
        "lighter answer",
    ));
    let mut runtime = demotable_runtime(main.clone());

    let (first, blocks) = run_turn_reporting(&mut runtime, "turn one").await;
    assert_eq!(first, Ok(2), "the spent ladder costs one extra iteration, not the turn");
    let overrides = main.overrides();
    assert_eq!(overrides.len(), 2, "exactly one demotion: {overrides:?}");
    assert_eq!(overrides[0], None, "the turn starts on the model the person chose");
    assert!(
        overrides[1].as_deref().is_some_and(|model| model.contains("sonnet")),
        "an Opus turn continues on Sonnet, got {overrides:?}"
    );
    assert!(history_text(&runtime).contains("lighter answer"));
    assert_eq!(
        warn_texts(&blocks).len(),
        1,
        "the demotion is announced once: {blocks:?}"
    );
    let (failed, first_token) = (
        main.mark("failed").expect("the heavy tier failed"),
        main.mark("first token").expect("the lighter tier answered"),
    );
    eprintln!(
        "[t-15565] spent 5xx ladder handed up -> first token on the lighter tier: {:?}",
        first_token.duration_since(failed)
    );

    // The demotion is the turn's, not the session's.
    let (second, _) = run_turn_reporting(&mut runtime, "turn two").await;
    assert_eq!(second, Ok(2));
    let overrides = main.overrides();
    assert_eq!(overrides.len(), 4, "{overrides:?}");
    assert_eq!(
        overrides[2], None,
        "the next turn starts on the model the person chose"
    );
    assert!(overrides[3].is_some(), "and gets its own one demotion");
}

/// (2) Nothing that is not a provider dropping the request is demoted: a 400 fails the
/// turn as it did, on its first and only request.
#[tokio::test]
async fn a_4xx_still_fails_the_turn_without_a_demotion() {
    let _env = hermetic_env();
    let main = Arc::new(ServerFaultingAsyncApi::new(
        "provider transport: api returned 400 Bad Request (invalid_request_error): messages.3.content: invalid",
        ProviderErrorClass::NonRetryable,
        "never",
    ));
    let mut runtime = demotable_runtime(main.clone());

    let (result, blocks) = run_turn_reporting(&mut runtime, "hi").await;
    let error = result.expect_err("a 400 fails the turn");
    assert!(error.contains("400 Bad Request"), "{error}");
    assert_eq!(main.overrides(), vec![None], "no second request, no lighter tier");
    assert!(warn_texts(&blocks).is_empty(), "{blocks:?}");
}

/// (3) The demotion says what a 529's says, from the same table: the two turns' warn
/// lines are the same words for the same models.
#[tokio::test]
async fn a_5xx_demotion_is_announced_in_the_words_a_529_demotion_is() {
    let _env = hermetic_env();
    let warns_of = |text: &'static str, class: ProviderErrorClass| async move {
        let main = Arc::new(ServerFaultingAsyncApi::new(text, class, "lighter answer"));
        let mut runtime = demotable_runtime(main);
        let (result, blocks) = run_turn_reporting(&mut runtime, "hi").await;
        assert_eq!(result, Ok(2), "{text}");
        warn_texts(&blocks)
    };
    // Keyword-free text for the 529: the escape reads the class, and the retry
    // ladder in front of it stays out of a test that is not about it.
    let overloaded = warns_of(
        "simulated provider overload (test)",
        ProviderErrorClass::provider_overloaded(None),
    )
    .await;
    let dropped = warns_of(SPENT_API_ERROR_FRAME, ProviderErrorClass::Transient).await;

    assert_eq!(overloaded.len(), 1, "{overloaded:?}");
    assert_eq!(dropped, overloaded, "one warn table for both walls");
}

/// (4) A lighter tier that is dropped too escalates the way a 529 does: with nowhere
/// else to go the turn ends after exactly one demotion; with a cross-provider client
/// installed the turn changes provider and completes there.
#[tokio::test]
async fn a_5xx_on_the_lighter_tier_too_ends_the_turn_or_changes_provider() {
    let _env = hermetic_env();

    let alone = Arc::new(
        ServerFaultingAsyncApi::new(
            SPENT_API_ERROR_FRAME,
            ProviderErrorClass::Transient,
            "never",
        )
        .also_failing_on_the_lighter_tier(),
    );
    let mut runtime = demotable_runtime(alone.clone());
    let (result, _) = run_turn_reporting(&mut runtime, "hi").await;
    assert!(result.is_err(), "nowhere left to go: the turn ends");
    assert_eq!(alone.overrides().len(), 2, "one demotion, never a second");

    let main = Arc::new(
        ServerFaultingAsyncApi::new(
            SPENT_API_ERROR_FRAME,
            ProviderErrorClass::Transient,
            "never",
        )
        .also_failing_on_the_lighter_tier(),
    );
    let mut runtime = demotable_runtime(main.clone());
    let fallback = Arc::new(RecordingAnswerAsyncApi::new("fallback answer"));
    runtime.set_quota_fallback_client(Some((fallback.clone(), "gpt-peer-x".to_string())));
    let (result, blocks) = run_turn_reporting(&mut runtime, "hi").await;
    assert_eq!(result, Ok(3), "heavy, lighter, then the other provider");
    assert_eq!(main.overrides().len(), 2);
    assert_eq!(fallback.call_count(), 1, "the other provider carried the turn");
    assert!(history_text(&runtime).contains("fallback answer"));
    assert_eq!(warn_texts(&blocks).len(), 2, "the demotion, then the swap: {blocks:?}");
}

/// (5) The headless `run_turn` road takes the same escape: it has its own decision
/// site, and a spent 5xx ladder must not die there while the streaming turn lives.
#[test]
fn sync_run_turn_demotes_a_spent_5xx_ladder() {
    struct ServerFaultingSyncApi;
    impl ApiClient for ServerFaultingSyncApi {
        fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            if request.model_override.is_none() {
                return Err(RuntimeError::with_provider_error_class(
                    SPENT_API_ERROR_FRAME,
                    ProviderErrorClass::Transient,
                ));
            }
            Ok(vec![
                AssistantEvent::TextDelta("lighter sync answer".to_string()),
                AssistantEvent::StopReason("end_turn".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
    }

    let _env = hermetic_env();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ServerFaultingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-opus-5");

    let summary = runtime
        .run_turn("hi", None)
        .expect("the sync turn continues one tier down, not fails");

    assert!(summary.iterations >= 2, "the sync loop re-requested one tier down");
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
    assert!(history.contains("lighter sync answer"), "got {history:?}");
}
