//! Regression: interactive `/compact` must not freeze the TUI.
//!
//! [`ConversationRuntime::compact_streaming`] is the non-blocking sibling of
//! [`ConversationRuntime::compact`] used by the `/compact` slash command. These
//! tests pin the two guarantees the freeze fix rests on:
//!
//! 1. With an [`AsyncApiClient`] installed, the summary round-trip is driven
//!    through it (it await-suspends) — never the synchronous `ApiClient::stream`
//!    that would block the drive-loop task — and a "Compacting…" notice is
//!    emitted up front so the user sees progress.
//! 2. With no async client (headless `-p`), it falls back to the exact same
//!    synchronous path as `compact`, producing an identical compaction result.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use runtime::message_stream::types::BlockId;
use runtime::message_stream::{BlockIdGen, RenderBlock};
use runtime::session::Session;
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, CompactionConfig, ContentBlock,
    ConversationRuntime, MessageRole, PermissionMode, PermissionPolicy, RuntimeError,
    StaticToolExecutor,
};
use tokio::sync::mpsc;

/// Sync client used only to build the session via `run_turn`. Its reply lacks a
/// `<summary>` block, so if compaction ever fell back to the synchronous summary
/// path it would trip the local summarizer instead — which the async-marker
/// assertion below would catch.
struct PlainSyncApi;
impl ApiClient for PlainSyncApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(vec![
            AssistantEvent::TextDelta("done".to_string()),
            AssistantEvent::MessageStop,
        ])
    }
}

/// Sync summarizer returning a well-formed `<summary>` block — used for the
/// headless-parity test where the synchronous path IS the path under test.
struct SummarySyncApi;
impl ApiClient for SummarySyncApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(vec![
            AssistantEvent::TextDelta(
                "<summary>\n- Current state: compacted via sync path.\n</summary>".to_string(),
            ),
            AssistantEvent::MessageStop,
        ])
    }
}

/// Async client returning a distinctively-marked `<summary>` so a test can prove
/// the summary round-trip went through the async (await-suspending) path.
struct SummaryAsyncApi {
    calls: AtomicUsize,
}
impl AsyncApiClient for SummaryAsyncApi {
    fn stream_async<'a>(
        &'a self,
        _request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![
                AssistantEvent::TextDelta(
                    "<summary>\n- Current state: compacted via async path.\n</summary>".to_string(),
                ),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

/// Async client that records each summary request so the focus-forwarding
/// contract can be checked independently of the selected request shape.
struct CapturingAsyncApi {
    requests: std::sync::Mutex<Vec<ApiRequest>>,
}
impl AsyncApiClient for CapturingAsyncApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.lock().expect("mutex").push(request);
            Ok(vec![
                AssistantEvent::TextDelta(
                    "<summary>\n- Current state: compacted via async path.\n</summary>".to_string(),
                ),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

fn fresh_id_gen() -> BlockIdGen {
    BlockIdGen(Arc::new(AtomicU64::new(0)))
}

const COMPACT_CONFIG: CompactionConfig = CompactionConfig {
    preserve_recent_messages: 2,
    max_estimated_tokens: 1,
};

fn compactable_runtime<C: ApiClient>(api: C) -> ConversationRuntime<C, StaticToolExecutor> {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        api,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.run_turn("a", None).expect("turn a");
    runtime.run_turn("b", None).expect("turn b");
    runtime.run_turn("c", None).expect("turn c");
    runtime
}

async fn drain(mut rx: mpsc::Receiver<RenderBlock>) -> Vec<RenderBlock> {
    let mut blocks = Vec::new();
    while let Some(block) = rx.recv().await {
        blocks.push(block);
    }
    blocks
}

fn saw_compacting_notice(blocks: &[RenderBlock]) -> bool {
    blocks.iter().any(|block| {
        matches!(
            block,
            RenderBlock::System { text, .. } if text.starts_with("Compacting conversation")
        )
    })
}

#[tokio::test]
async fn compact_streaming_routes_summary_through_async_client_and_emits_notice() {
    let mut runtime = compactable_runtime(PlainSyncApi);
    let async_impl = Arc::new(SummaryAsyncApi {
        calls: AtomicUsize::new(0),
    });
    let async_client: Arc<dyn AsyncApiClient> = async_impl.clone();
    runtime.set_async_api_client(async_client);

    let (tx, rx) = mpsc::channel::<RenderBlock>(64);
    let ids = fresh_id_gen();
    let result = runtime
        .compact_streaming(COMPACT_CONFIG, &tx, &ids, None)
        .await;
    drop(tx);
    let blocks = drain(rx).await;

    // The async client produced exactly one summary round-trip — proving the
    // await-suspending path ran, not the blocking sync `ApiClient::stream`.
    assert_eq!(async_impl.calls.load(Ordering::SeqCst), 1);
    assert!(
        result.summary.contains("compacted via async path"),
        "summary should come from the async client, got {:?}",
        result.summary
    );
    assert!(
        result.removed_message_count > 0,
        "compaction should have removed messages"
    );
    assert!(
        saw_compacting_notice(&blocks),
        "expected a 'Compacting…' start notice, got {blocks:?}"
    );
}

#[tokio::test]
async fn compact_streaming_without_async_client_matches_sync_compact() {
    // No async client installed → compact_streaming must take the exact same
    // synchronous path as `compact`, producing an identical compaction result
    // (the headless `-p` contract is unchanged).
    let mut sync_runtime = compactable_runtime(SummarySyncApi);
    let sync_result = sync_runtime.compact(COMPACT_CONFIG, None);

    let mut streaming_runtime = compactable_runtime(SummarySyncApi);
    let (tx, rx) = mpsc::channel::<RenderBlock>(64);
    let ids = fresh_id_gen();
    let streaming_result = streaming_runtime
        .compact_streaming(COMPACT_CONFIG, &tx, &ids, None)
        .await;
    drop(tx);
    let blocks = drain(rx).await;

    assert_eq!(streaming_result.summary, sync_result.summary);
    assert_eq!(
        streaming_result.removed_message_count,
        sync_result.removed_message_count
    );
    assert_eq!(
        streaming_result.compacted_session.messages.len(),
        sync_result.compacted_session.messages.len()
    );
    assert!(
        saw_compacting_notice(&blocks),
        "the progress notice is surfaced even on the sync fallback, got {blocks:?}"
    );
}

#[tokio::test]
async fn compact_streaming_threads_focus_into_async_summary_request() {
    // This integration test owns async focus forwarding. The sync request-shape
    // tests separately pin where each supported compaction mode stores it.
    let focus = "the retry budget floor";
    let mut runtime = compactable_runtime(PlainSyncApi);
    let api = Arc::new(CapturingAsyncApi {
        requests: std::sync::Mutex::new(Vec::new()),
    });
    runtime.set_async_api_client(api.clone() as Arc<dyn AsyncApiClient>);

    let (tx, rx) = mpsc::channel::<RenderBlock>(64);
    let ids = fresh_id_gen();
    let result = runtime
        .compact_streaming(COMPACT_CONFIG, &tx, &ids, Some(focus))
        .await;
    drop(tx);
    let _ = drain(rx).await;

    let requests = api.requests.lock().expect("mutex");
    assert_eq!(requests.len(), 1, "focused compaction still uses the async API path");
    let request = &requests[0];
    let focus_in_final_user_instruction = request.messages.last().is_some_and(|message| {
        message.role == MessageRole::User
            && matches!(
                message.blocks.first(),
                Some(ContentBlock::Text { text }) if text.contains(focus)
            )
    });
    let focus_in_system_prompt = request
        .system_prompt
        .iter()
        .any(|prompt| prompt.contains(focus));
    assert!(
        focus_in_final_user_instruction || focus_in_system_prompt,
        "focus directive must reach the async summary request"
    );
    assert!(
        result.removed_message_count > 0,
        "focused compaction should have removed messages"
    );
}
