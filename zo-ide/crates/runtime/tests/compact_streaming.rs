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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use runtime::message_stream::types::{BlockId, StreamPhase};
use runtime::message_stream::{BlockIdGen, RenderBlock, SystemLevel};
use runtime::permission::{
    PermissionDecision, PermissionError, PermissionPrompter, PermissionRequest,
};
use runtime::session::Session;
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, CompactionConfig, ContentBlock,
    ConversationMessage, ConversationRuntime, MessageRole, PermissionMode, PermissionPolicy,
    RuntimeError, StaticToolExecutor, TokenUsage,
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
    let done = runtime
        .compact_streaming(COMPACT_CONFIG, &tx, &ids, None)
        .await;
    drop(tx);
    let blocks = drain(rx).await;
    assert_eq!(done.local_summary_reason, None, "the model's summary came");
    let result = done.result;

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
        .await
        .result;
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
        .await
        .result;
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

/// Async client whose summary round-trip fails, the way a provider that
/// rejects the summary request (a conversation too long for it, a wall) does.
struct FailingAsyncApi;
impl AsyncApiClient for FailingAsyncApi {
    fn stream_async<'a>(
        &'a self,
        _request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move { Err(RuntimeError::new("prompt is too long: 1200000 tokens > 1000000 maximum")) })
    }
}

#[tokio::test]
async fn compact_streaming_says_why_the_local_summary_stands_in() {
    // t-10956: when the model's summary does not come, `/compact` still folds
    // the conversation with the deterministic local summary — and says why,
    // instead of handing over a thinner summary in silence.
    let mut runtime = compactable_runtime(PlainSyncApi);
    runtime.set_async_api_client(Arc::new(FailingAsyncApi) as Arc<dyn AsyncApiClient>);

    let (tx, rx) = mpsc::channel::<RenderBlock>(64);
    let ids = fresh_id_gen();
    let done = runtime
        .compact_streaming(COMPACT_CONFIG, &tx, &ids, None)
        .await;
    drop(tx);
    let _ = drain(rx).await;

    assert!(
        done.result.removed_message_count > 0,
        "the local summary still folds the conversation"
    );
    let reason = done.local_summary_reason.expect("the reason the local summary stood in");
    assert!(
        reason.contains("prompt is too long"),
        "the reason names what the provider said: {reason}"
    );
}

// ── t-68694: a compaction whose summary never comes ─────────────────────────
//
// A zo pane compacted 393 messages in the middle of a turn and then showed
// `Working` for 51 minutes with nothing behind it: the summary stream was
// awaited with no deadline, and the screen dropped the compaction's progress.
// These tests wait the summary out on a paused clock, so an hour of waiting
// costs no real time.

/// Longer than any deadline the summary may have: a wait still going this
/// long is a wait with none.
const AN_HOUR: Duration = Duration::from_secs(60 * 60);

/// What a finished helper hands back to the turn — the result the turn must
/// still hold after it compacted.
const HELPER_REPORT: &str = "C68694 helper report: the parser reads its tokens in three passes.";

/// The summary a willing summarizer writes.
const SUMMARY: &str = "<summary>\n- Current state: C68694 compacted mid-turn.\n</summary>";

/// A reconnect notice the live client puts on the request's own channel.
const RECONNECT_NOTICE: &str = "transient provider error; reconnecting in 2s (attempt 1/7)";

/// Whether `request` is a compaction's summary request, in either of the two
/// shapes it is sent in (the summary instruction as the system prompt, or as
/// the last user message of the live conversation).
fn is_summary_request(request: &ApiRequest) -> bool {
    let instruction = runtime::compaction_system_prompt(None);
    request
        .system_prompt
        .iter()
        .any(|prompt| prompt.contains(&instruction))
        || request.messages.last().is_some_and(|message| {
            message.blocks.iter().any(
                |block| matches!(block, ContentBlock::Text { text } if text.contains(&instruction)),
            )
        })
}

/// Sets its flag when the future holding it is dropped: the summary request
/// was stopped, not left running behind the turn.
struct DropFlag(Arc<AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn usage(input_tokens: u32, cache_read_input_tokens: u32) -> TokenUsage {
    TokenUsage {
        input_tokens,
        output_tokens: 20,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens,
        output_tokens_details: None,
    }
}

/// A summary request never answers. A turn's first request calls a helper
/// with the context near full; every later one answers at once.
struct EndlessSummaryApi {
    turn_requests: AtomicUsize,
    summary_stopped: Arc<AtomicBool>,
}
impl AsyncApiClient for EndlessSummaryApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            if is_summary_request(&request) {
                let _stopped = DropFlag(Arc::clone(&self.summary_stopped));
                return std::future::pending::<Result<Vec<AssistantEvent>, RuntimeError>>().await;
            }
            if self.turn_requests.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "toolu_c68694".to_string(),
                        name: "Agent".to_string(),
                        input: r#"{"description":"survey the parser","prompt":"Read the parser and report."}"#
                            .to_string(),
                    },
                    AssistantEvent::Usage(usage(1_000, 119_000)),
                    AssistantEvent::MessageStop,
                ]);
            }
            Ok(vec![
                AssistantEvent::TextDelta("C68694 the turn goes on".to_string()),
                AssistantEvent::Usage(usage(2_000, 0)),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

/// A turn's request answers at once with the context near full. A summary
/// request goes the way the live client takes one: a reconnect notice and a
/// retry phase on the request's own channel, the summary's text, then the
/// summary.
struct ReconnectingSummaryApi;
impl AsyncApiClient for ReconnectingSummaryApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            if !is_summary_request(&request) {
                return Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::Usage(usage(1_000, 119_000)),
                    AssistantEvent::MessageStop,
                ]);
            }
            let _ = render_tx
                .send(RenderBlock::System {
                    id: text_block_id,
                    level: SystemLevel::Warn,
                    text: RECONNECT_NOTICE.to_string(),
                })
                .await;
            let _ = render_tx
                .send(RenderBlock::StreamPhase(StreamPhase::Retrying {
                    attempt: 1,
                    delay_secs: 2,
                }))
                .await;
            let _ = render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: SUMMARY.to_string(),
                    done: true,
                })
                .await;
            Ok(vec![
                AssistantEvent::TextDelta(SUMMARY.to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

struct AllowPrompter;
impl PermissionPrompter for AllowPrompter {
    fn decide<'a>(
        &'a self,
        _request: PermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PermissionDecision, PermissionError>> + Send + 'a>> {
        Box::pin(async { Ok(PermissionDecision::Allow) })
    }
}

/// Eight earlier messages, so a compaction has something to fold besides the
/// turn's own last four.
fn earlier_conversation() -> Session {
    let mut session = Session::new();
    let mut messages = Vec::new();
    for exchange in 0..4 {
        messages.push(ConversationMessage::user_text(format!("C68694 question {exchange}")));
        messages.push(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: format!("C68694 answer {exchange}"),
        }]));
    }
    session.messages = Arc::new(messages);
    session
}

fn turn_runtime(
    async_client: Arc<dyn AsyncApiClient>,
) -> ConversationRuntime<PlainSyncApi, StaticToolExecutor> {
    let mut runtime = ConversationRuntime::new(
        earlier_conversation(),
        PlainSyncApi,
        StaticToolExecutor::new().register("Agent", |_input| Ok(HELPER_REPORT.to_string())),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    )
    .with_auto_compaction_input_tokens_threshold(100_000);
    runtime.set_async_api_client(async_client);
    runtime
}

fn system_rows(blocks: &[RenderBlock]) -> Vec<(SystemLevel, &str)> {
    blocks
        .iter()
        .filter_map(|block| match block {
            RenderBlock::System { level, text, .. } => Some((*level, text.as_str())),
            _ => None,
        })
        .collect()
}

/// The person's case: the helper finished, the context crossed the line, and
/// the turn compacted — and the summary never came. The wait now ends at its
/// deadline: the summary request is stopped, the local summary folds the
/// conversation, one line says what happened, and the turn goes on to its
/// answer with the helper's report still in the conversation.
#[tokio::test(start_paused = true)]
async fn an_endless_summary_inside_a_turn_is_cut_at_its_deadline_and_the_turn_goes_on() {
    let summary_stopped = Arc::new(AtomicBool::new(false));
    let api = Arc::new(EndlessSummaryApi {
        turn_requests: AtomicUsize::new(0),
        summary_stopped: Arc::clone(&summary_stopped),
    });
    let mut runtime = turn_runtime(api.clone() as Arc<dyn AsyncApiClient>);
    let (tx, rx) = mpsc::channel::<RenderBlock>(256);
    let collected = tokio::spawn(drain(rx));

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(AllowPrompter);
    let finished =
        tokio::time::timeout(AN_HOUR, runtime.run_turn_streaming("C68694 survey", tx, prompter))
            .await;
    let blocks = collected.await.expect("the channel drains");

    let summary = finished
        .expect("the turn was still waiting on the compaction's summary an hour later")
        .expect("the turn ends without an error");
    assert!(
        summary_stopped.load(Ordering::SeqCst),
        "the summary request that never ended is stopped, not left running"
    );
    let compaction = summary
        .auto_compaction
        .expect("the local summary still folds the conversation");
    assert!(compaction.removed_message_count > 0, "{compaction:?}");

    let rows = system_rows(&blocks);
    let said: Vec<&(SystemLevel, &str)> = rows
        .iter()
        .filter(|(_, text)| text.contains("summary did not come"))
        .collect();
    assert_eq!(said.len(), 1, "one line says what happened: {rows:?}");
    let (level, line) = said[0];
    assert_eq!(*level, SystemLevel::Warn, "{line}");
    assert!(!line.contains('\n'), "one line: {line:?}");
    assert!(
        line.contains("not done after") && line.contains("local summary"),
        "the line names the deadline and what stood in: {line}"
    );

    assert_eq!(
        api.turn_requests.load(Ordering::SeqCst),
        2,
        "the turn asks again after the compaction"
    );
    let messages = &runtime.session().messages;
    assert!(
        messages.iter().any(|message| message.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::ToolResult { output, .. } if output.contains(HELPER_REPORT)
        ))),
        "the finished helper's report is still in the conversation"
    );
    assert!(
        messages.last().is_some_and(|message| message.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::Text { text } if text.contains("C68694 the turn goes on")
        ))),
        "the turn reached its answer after the compaction"
    );
}

/// While a turn compacts, the status line is told so: the compaction is
/// bracketed by its own two phases, and inside them the summary request is
/// announced, the provider's reconnect notice and retry phase reach the turn's
/// channel, and the summary's first characters are counted at once.
#[tokio::test]
async fn an_in_turn_compaction_names_its_wait_on_the_turns_channel() {
    let mut runtime = turn_runtime(Arc::new(ReconnectingSummaryApi) as Arc<dyn AsyncApiClient>);
    let (tx, rx) = mpsc::channel::<RenderBlock>(256);
    let collected = tokio::spawn(drain(rx));
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(AllowPrompter);
    let summary = runtime
        .run_turn_streaming("C68694 survey", tx, prompter)
        .await
        .expect("the turn ends without an error");
    let blocks = collected.await.expect("the channel drains");
    assert!(summary.auto_compaction.is_some(), "the turn compacted");

    let after = |from: usize, found: &dyn Fn(&RenderBlock) -> bool| {
        blocks
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, block)| found(block))
            .map(|(index, _)| index)
    };
    let notice = after(0, &|block| {
        matches!(block, RenderBlock::System { text, .. } if text.starts_with("Compacting conversation"))
    })
    .expect("the compaction's start notice");
    let started = after(notice, &|block| {
        matches!(block, RenderBlock::StreamPhase(StreamPhase::Compacting))
    })
    .unwrap_or_else(|| panic!("no phase tells the status line the turn compacts: {blocks:?}"));
    let sent = after(started, &|block| {
        matches!(block, RenderBlock::StreamPhase(StreamPhase::RequestSent { attempt: 1 }))
    })
    .unwrap_or_else(|| panic!("the summary request is not announced: {blocks:?}"));
    let reconnect = after(sent, &|block| {
        matches!(block, RenderBlock::System { level: SystemLevel::Warn, text, .. } if text == RECONNECT_NOTICE)
    })
    .unwrap_or_else(|| panic!("the summary's reconnect notice is dropped: {blocks:?}"));
    let retrying = after(sent, &|block| {
        matches!(block, RenderBlock::StreamPhase(StreamPhase::Retrying { attempt: 1, .. }))
    })
    .unwrap_or_else(|| panic!("the summary's retry phase is dropped: {blocks:?}"));
    let counted = after(sent, &|block| {
        matches!(block, RenderBlock::CompactionProgress { streamed_chars } if *streamed_chars > 0)
    })
    .unwrap_or_else(|| panic!("the summary's first characters are not counted: {blocks:?}"));
    let ended = after(counted.max(reconnect).max(retrying), &|block| {
        matches!(block, RenderBlock::StreamPhase(StreamPhase::CompactionEnded))
    })
    .unwrap_or_else(|| panic!("no phase tells the status line the compaction ended: {blocks:?}"));
    after(ended, &|block| {
        matches!(block, RenderBlock::System { text, .. } if text.starts_with("Compacted conversation"))
    })
    .unwrap_or_else(|| panic!("the done notice follows the end: {blocks:?}"));
}

/// `/compact` waits on the same summary request, and is cut at the same
/// deadline: the local summary folds the conversation and the report says why.
#[tokio::test(start_paused = true)]
async fn compact_streaming_stops_waiting_for_a_summary_that_never_ends() {
    let mut runtime = compactable_runtime(PlainSyncApi);
    let summary_stopped = Arc::new(AtomicBool::new(false));
    runtime.set_async_api_client(Arc::new(EndlessSummaryApi {
        turn_requests: AtomicUsize::new(0),
        summary_stopped: Arc::clone(&summary_stopped),
    }) as Arc<dyn AsyncApiClient>);

    let (tx, rx) = mpsc::channel::<RenderBlock>(64);
    let ids = fresh_id_gen();
    let done = tokio::time::timeout(
        AN_HOUR,
        runtime.compact_streaming(COMPACT_CONFIG, &tx, &ids, None),
    )
    .await
    .expect("/compact was still waiting on the summary an hour later");
    drop(tx);
    let _ = drain(rx).await;

    assert!(summary_stopped.load(Ordering::SeqCst), "the summary request is stopped");
    assert!(
        done.result.removed_message_count > 0,
        "the local summary still folds the conversation"
    );
    let reason = done.local_summary_reason.expect("the reason the local summary stood in");
    assert!(reason.contains("not done after"), "the reason names the deadline: {reason}");
}

// ── t-69472: the sync road's summary has the same deadline ──────────────────
//
// A helper's own turns run on the sync road (`run_turn`), and so do their
// compactions: the summary was waited on with no deadline at all. The client
// is now asked to give the request up at t-68694's deadline
// (`ApiClient::stream_within`); the local summary folds the conversation, one
// line on stderr — the sync road's only screen — says so, and the turn goes on.

/// Set in the child run of the sync turn's test: the child runs the turn, and
/// the parent reads the child's stderr.
const SYNC_TURN_CHILD_ENV: &str = "ZO_C69472_SYNC_TURN_CHILD";

/// The test the child runs, by its exact name.
const SYNC_TURN_TEST: &str =
    "an_endless_summary_in_a_sync_turn_is_cut_at_its_deadline_and_the_turn_goes_on";

/// How long the child's turn may take before it counts as stuck: the summary
/// is waited out on a paused clock, so a turn that ends ends at once.
const STUCK_AFTER: Duration = Duration::from_secs(30);

/// A sync client the way a helper's own client is one: it drives each request
/// as a future on a runtime of its own — here one whose clock is paused, so
/// ten minutes cost no real time. A turn's first request calls a helper with
/// the context near full; every later one answers at once. A summary request
/// never ends: `stream` waits on it for ever, and `stream_within` gives it up
/// when its deadline passes on that clock.
struct EndlessSummarySyncApi {
    clock: tokio::runtime::Runtime,
    turn_requests: Arc<AtomicUsize>,
    /// The deadline the summary request was given, once it was asked.
    summary_deadline: Arc<std::sync::Mutex<Option<Duration>>>,
}

impl EndlessSummarySyncApi {
    fn answer(&self) -> Vec<AssistantEvent> {
        if self.turn_requests.fetch_add(1, Ordering::SeqCst) == 0 {
            return vec![
                AssistantEvent::ToolUse {
                    id: "toolu_c69472".to_string(),
                    name: "Agent".to_string(),
                    input: r#"{"description":"survey the parser","prompt":"Read the parser and report."}"#
                        .to_string(),
                },
                AssistantEvent::Usage(usage(1_000, 119_000)),
                AssistantEvent::MessageStop,
            ];
        }
        vec![
            AssistantEvent::TextDelta("C69472 the turn goes on".to_string()),
            AssistantEvent::Usage(usage(2_000, 0)),
            AssistantEvent::MessageStop,
        ]
    }
}

impl ApiClient for EndlessSummarySyncApi {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        if is_summary_request(&request) {
            // A call that only blocks cannot be given up: it waits for ever.
            return self.clock.block_on(std::future::pending());
        }
        Ok(self.answer())
    }

    fn stream_within(
        &mut self,
        request: ApiRequest,
        deadline: Duration,
    ) -> Result<Option<Vec<AssistantEvent>>, RuntimeError> {
        if !is_summary_request(&request) {
            return self.stream(request).map(Some);
        }
        *self.summary_deadline.lock().expect("mutex") = Some(deadline);
        let waited = self.clock.block_on(async {
            let asked = tokio::time::Instant::now();
            let _ = tokio::time::timeout(deadline, std::future::pending::<()>()).await;
            asked.elapsed()
        });
        assert!(waited >= deadline, "the summary was given up {waited:?} in, before its deadline");
        Ok(None)
    }
}

/// The child's run: a sync turn whose compaction's summary never ends, on a
/// thread of its own, so a turn that never ends fails the run instead of
/// holding it.
fn run_a_sync_turn_with_an_endless_summary() {
    let turn_requests = Arc::new(AtomicUsize::new(0));
    let summary_deadline = Arc::new(std::sync::Mutex::new(None));
    let api = EndlessSummarySyncApi {
        clock: tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .expect("a runtime on a paused clock"),
        turn_requests: Arc::clone(&turn_requests),
        summary_deadline: Arc::clone(&summary_deadline),
    };
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut runtime = ConversationRuntime::new(
            earlier_conversation(),
            api,
            StaticToolExecutor::new().register("Agent", |_input| Ok(HELPER_REPORT.to_string())),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_auto_compaction_input_tokens_threshold(100_000);
        let summary = runtime.run_turn("C69472 survey", None);
        let _ = done_tx.send((summary, Arc::clone(&runtime.session().messages)));
    });
    let (summary, messages) = done_rx
        .recv_timeout(STUCK_AFTER)
        .expect("the sync turn was still waiting on the compaction's summary");

    let summary = summary.expect("the turn ends without an error");
    assert_eq!(
        *summary_deadline.lock().expect("mutex"),
        Some(runtime::COMPACTION_SUMMARY_DEADLINE),
        "the summary request is given t-68694's deadline"
    );
    let compaction = summary
        .auto_compaction
        .expect("the local summary still folds the conversation");
    assert!(compaction.removed_message_count > 0, "{compaction:?}");
    assert_eq!(
        turn_requests.load(Ordering::SeqCst),
        2,
        "the turn asks again after the compaction"
    );
    assert!(
        messages.iter().any(|message| message.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::ToolResult { output, .. } if output.contains(HELPER_REPORT)
        ))),
        "the finished helper's report is still in the conversation"
    );
    assert!(
        messages.last().is_some_and(|message| message.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::Text { text } if text.contains("C69472 the turn goes on")
        ))),
        "the turn reached its answer after the compaction"
    );
}

/// The person's case on the sync road: a helper's own turn compacted, and the
/// summary never came. The wait ends at t-68694's deadline there too — the
/// client gives the request up, the local summary folds the conversation, one
/// line says what happened, and the turn goes on to its answer with the
/// finished helper's report kept.
///
/// The sync road has no render channel: its line is a `[zo]` line on stderr,
/// so the test runs the turn in a child of its own binary and reads it there.
#[test]
fn an_endless_summary_in_a_sync_turn_is_cut_at_its_deadline_and_the_turn_goes_on() {
    if std::env::var_os(SYNC_TURN_CHILD_ENV).is_some() {
        run_a_sync_turn_with_an_endless_summary();
        return;
    }
    let child = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", SYNC_TURN_TEST, "--nocapture", "--test-threads=1"])
        .env(SYNC_TURN_CHILD_ENV, "1")
        .output()
        .expect("the sync turn runs in a child");
    let stderr = String::from_utf8_lossy(&child.stderr);
    assert!(
        child.status.success(),
        "the sync turn did not end well ({}):\n{stderr}",
        child.status
    );
    let said: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("summary did not come"))
        .collect();
    assert_eq!(said.len(), 1, "one line says what happened:\n{stderr}");
    let line = said[0];
    assert!(
        line.starts_with("[zo] Compaction: ")
            && line.contains("not done after")
            && line.ends_with("the local summary stands in"),
        "the line names the deadline and what stood in: {line}"
    );
}

/// What a turn's own request answers once it is done.
const LONG_ANSWER: &str = "C69472 a long answer";

/// Longer than a summary's deadline: a long reasoning turn's request.
fn longer_than_the_summary_deadline() -> Duration {
    runtime::COMPACTION_SUMMARY_DEADLINE + Duration::from_secs(60)
}

fn a_long_answer() -> Vec<AssistantEvent> {
    vec![
        AssistantEvent::TextDelta(LONG_ANSWER.to_string()),
        AssistantEvent::Usage(usage(2_000, 0)),
        AssistantEvent::MessageStop,
    ]
}

fn ends_with_the_long_answer(messages: &[ConversationMessage]) -> bool {
    messages.last().is_some_and(|message| {
        message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text.contains(LONG_ANSWER))
        })
    })
}

/// A sync client whose turn request takes longer than a summary's deadline
/// on a paused clock, and then answers. Every deadline it is given is kept.
struct SlowTurnSyncApi {
    clock: tokio::runtime::Runtime,
    deadlines: Arc<std::sync::Mutex<Vec<Duration>>>,
    /// How long the turn's request took, on the paused clock.
    took: Arc<std::sync::Mutex<Option<Duration>>>,
}

impl ApiClient for SlowTurnSyncApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let took = self.clock.block_on(async {
            let asked = tokio::time::Instant::now();
            tokio::time::sleep(longer_than_the_summary_deadline()).await;
            asked.elapsed()
        });
        *self.took.lock().expect("mutex") = Some(took);
        Ok(a_long_answer())
    }

    fn stream_within(
        &mut self,
        request: ApiRequest,
        deadline: Duration,
    ) -> Result<Option<Vec<AssistantEvent>>, RuntimeError> {
        self.deadlines.lock().expect("mutex").push(deadline);
        self.stream(request).map(Some)
    }
}

/// t-69472: the deadline is the summary's alone. A sync turn's own request —
/// a long reasoning turn's, here longer than the summary's deadline — is
/// given none, and its answer is the turn's whenever it comes.
#[test]
fn a_sync_turn_request_longer_than_the_summary_deadline_is_not_cut() {
    let deadlines = Arc::new(std::sync::Mutex::new(Vec::new()));
    let took = Arc::new(std::sync::Mutex::new(None));
    let mut conversation = ConversationRuntime::new(
        earlier_conversation(),
        SlowTurnSyncApi {
            clock: tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .start_paused(true)
                .build()
                .expect("a runtime on a paused clock"),
            deadlines: Arc::clone(&deadlines),
            took: Arc::clone(&took),
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    conversation
        .run_turn("C69472 think it through", None)
        .expect("the long turn ends with its answer");

    assert!(
        deadlines.lock().expect("mutex").is_empty(),
        "a turn's own request was given a deadline: {:?}",
        deadlines.lock().expect("mutex")
    );
    let took = took.lock().expect("mutex").expect("the turn asked");
    assert!(took > runtime::COMPACTION_SUMMARY_DEADLINE, "the request took {took:?}");
    assert!(
        ends_with_the_long_answer(&conversation.session().messages),
        "the answer that came after the summary's deadline is the turn's"
    );
}

/// An async client whose turn request takes longer than a summary's deadline
/// on the test's paused clock, and then answers.
struct SlowTurnAsyncApi {
    requests: AtomicUsize,
}
impl AsyncApiClient for SlowTurnAsyncApi {
    fn stream_async<'a>(
        &'a self,
        _request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(longer_than_the_summary_deadline()).await;
            Ok(a_long_answer())
        })
    }
}

/// t-69472: the same on the async road — t-68694's deadline holds the
/// summary's wait only, and a turn's request longer than it is neither cut
/// nor asked again.
#[tokio::test(start_paused = true)]
async fn a_streaming_turn_request_longer_than_the_summary_deadline_is_not_cut() {
    let api = Arc::new(SlowTurnAsyncApi {
        requests: AtomicUsize::new(0),
    });
    let mut conversation = turn_runtime(api.clone() as Arc<dyn AsyncApiClient>);
    let (tx, rx) = mpsc::channel::<RenderBlock>(256);
    let collected = tokio::spawn(drain(rx));
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(AllowPrompter);
    let asked = tokio::time::Instant::now();
    tokio::time::timeout(AN_HOUR, conversation.run_turn_streaming("C69472 think it through", tx, prompter))
        .await
        .expect("the long turn was still going an hour later")
        .expect("the long turn ends with its answer");
    let _ = collected.await;

    assert!(
        asked.elapsed() > runtime::COMPACTION_SUMMARY_DEADLINE,
        "the turn took {:?}",
        asked.elapsed()
    );
    assert_eq!(api.requests.load(Ordering::SeqCst), 1, "the long request is asked once");
    assert!(
        ends_with_the_long_answer(&conversation.session().messages),
        "the answer that came after the summary's deadline is the turn's"
    );
}

// ── t-69472: a summary that fails for another reason says so too ──────────

/// What the provider said when it refused the summary request: its own
/// words, which no line the person reads may carry.
const PROVIDER_SAID: &str = "prompt is too long: 1200000 tokens > 1000000 maximum";

/// A turn's request answers at once with the context near full. A summary
/// request is refused the way a provider refuses a conversation too long for
/// it, in the provider's own words.
struct RefusedSummaryApi;
impl AsyncApiClient for RefusedSummaryApi {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            if is_summary_request(&request) {
                return Err(RuntimeError::with_provider_error_class(
                    PROVIDER_SAID,
                    runtime::ProviderErrorClass::ContextOverflow,
                ));
            }
            Ok(vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::Usage(usage(1_000, 119_000)),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

/// t-69472: a summary that fails inside a turn for another reason than its
/// deadline — here the provider refuses the request — still lets the local
/// summary fold the conversation, and the turn says so in one line of its
/// own: the kind of failure only, never the provider's words, and not the
/// deadline's line.
#[tokio::test]
async fn an_in_turn_summary_that_fails_says_its_kind_in_one_line() {
    let mut conversation = turn_runtime(Arc::new(RefusedSummaryApi) as Arc<dyn AsyncApiClient>);
    let (tx, rx) = mpsc::channel::<RenderBlock>(256);
    let collected = tokio::spawn(drain(rx));
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(AllowPrompter);
    let summary = conversation
        .run_turn_streaming("C69472 survey", tx, prompter)
        .await
        .expect("the turn ends without an error");
    let blocks = collected.await.expect("the channel drains");
    assert!(
        summary.auto_compaction.is_some(),
        "the local summary folds the conversation"
    );

    let rows = system_rows(&blocks);
    let said: Vec<&(SystemLevel, &str)> = rows
        .iter()
        .filter(|(_, text)| text.contains("the local summary stands in"))
        .collect();
    assert_eq!(said.len(), 1, "one line says the local summary stood in: {rows:?}");
    let (level, line) = said[0];
    assert_eq!(*level, SystemLevel::Warn, "{line}");
    assert_eq!(
        *line,
        "Compaction: the model's summary could not be used (the conversation is too long for the summary request); the local summary stands in",
        "the line names the kind of failure, apart from the deadline's line"
    );
    assert!(!line.contains(PROVIDER_SAID), "the provider's words stay out of the line: {line}");
}
