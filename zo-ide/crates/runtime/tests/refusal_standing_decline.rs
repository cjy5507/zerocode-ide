//! t-15890 — the decline the person met on 09-29, in the catalog their build
//! ran with.
//!
//! The shipped catalog routes a `cyber` decline on Opus to Opus 4.8. The
//! person's runtime did not: discovery had published `opus → claude-opus-5-5`
//! as an alias row of its own — no `refusal_routes`, no `refusal_fallback` —
//! and the registry lets an earlier row shadow the shipped row of the same
//! alias, so the provider "routed `cyber` declines to no other model" (the
//! line on their screen) for every category on Opus 5.5. The registry is
//! process-global, so this is a binary of its own: it installs those rows once
//! and walks the person's turns on them — a `cyber` decline, then `continue`.
//!
//! The person's build reached this state because a discovery row shadowed the
//! shipped routes (t-16493); this test sets the state explicitly — the rows
//! carry a `refusal_routes` map that names no `cyber` — so it holds before and
//! after that fix.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use runtime::message_stream::types::BlockId;
use runtime::message_stream::RenderBlock;
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

/// The alias rows the person's `zo models --json` printed for Opus on 09-30 —
/// the discovered release — with the one thing their runtime lacked, said
/// outright: a `refusal_routes` map (the shipped catalog's key) that routes
/// `bio` and names no `cyber`. A category a published row's map leaves out is
/// routed nowhere, whatever the shipped row beneath it says.
const PUBLISHED_OPUS_ALIASES: &str = r#"{"aliases": [
    {"alias": "opus", "canonical": "claude-opus-5-5", "provider": "anthropic",
     "refusal_routes": {"bio": ["claude-opus-5", "openai-latest"]}},
    {"alias": "opus[1m]", "canonical": "claude-opus-5-5", "provider": "anthropic",
     "refusal_routes": {"bio": ["claude-opus-5", "openai-latest"]}},
    {"alias": "claude-opus", "canonical": "claude-opus-5-5", "provider": "anthropic",
     "refusal_routes": {"bio": ["claude-opus-5", "openai-latest"]}},
    {"alias": "claude-opus[1m]", "canonical": "claude-opus-5-5", "provider": "anthropic",
     "refusal_routes": {"bio": ["claude-opus-5", "openai-latest"]}}
]}"#;

/// What one request costs the fake provider — the number a person feels.
const REQUEST_LATENCY: Duration = Duration::from_millis(50);

static INSTALL: Once = Once::new();

/// Serializes the tests of this binary: `ZO_SHARED_RATE_COORD` is
/// process-global, and `api` reads the developer's live shared-quota
/// observation unless it is off.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The lock a test holds for its length, wrapped so that holding it across the
/// test's awaits is the intent and not a lint.
struct PersonsCatalog {
    _lock: std::sync::MutexGuard<'static, ()>,
}

/// Install the person's catalog and say what it makes of the decline.
fn the_persons_catalog() -> PersonsCatalog {
    let lock = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::set_var("ZO_SHARED_RATE_COORD", "0");
    INSTALL.call_once(|| api::refresh_model_registry_from_json(PUBLISHED_OPUS_ALIASES));
    assert!(!api::refusal_route_candidates("claude-opus-5-5", Some("bio")).is_empty());
    assert!(
        api::refusal_route_candidates("claude-opus-5-5", Some("cyber")).is_empty(),
        "the premise: on these rows the provider routes `cyber` on Opus 5.5 nowhere, and `bio` somewhere"
    );
    PersonsCatalog { _lock: lock }
}

/// Sync client that must never be called: the async seam is installed.
struct ExplodingSyncApi;

impl ApiClient for ExplodingSyncApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        panic!("sync ApiClient::stream must not be called when async seam is installed");
    }
}

/// Declines every request in one category, after [`REQUEST_LATENCY`], and
/// counts them.
struct Declining {
    calls: AtomicUsize,
    category: &'static str,
}

impl AsyncApiClient for Declining {
    fn stream_async<'a>(
        &'a self,
        _request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(REQUEST_LATENCY).await;
            let mut events = vec![AssistantEvent::StopReason("refusal".to_string())];
            runtime::push_refusal_category(&mut events, Some(self.category));
            events.push(AssistantEvent::MessageStop);
            Ok(events)
        })
    }
}

/// Denies whatever it is asked, and counts the questions. The count is a
/// static because the tests of this binary run one at a time under
/// [`ENV_LOCK`].
struct DenyPrompter;

static QUESTIONS_ASKED: AtomicUsize = AtomicUsize::new(0);

impl PermissionPrompter for DenyPrompter {
    fn decide<'a>(
        &'a self,
        _request: AsyncPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>>
    {
        QUESTIONS_ASKED.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(AsyncPermissionDecision::Deny) })
    }
}

/// One streaming turn of `words`: how many model round-trips it made, every
/// block the screen was sent, and how long it took.
async fn turn(
    runtime: &mut ConversationRuntime<ExplodingSyncApi, StaticToolExecutor>,
    words: &str,
) -> (usize, Vec<RenderBlock>, Duration) {
    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut blocks = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let started = Instant::now();
    let summary = runtime
        .run_turn_streaming(words, tx, prompter)
        .await
        .expect("a surfaced decline ends the turn cleanly");
    let took = started.elapsed();
    (summary.iterations, drain.await.expect("drain"), took)
}

fn system_lines(blocks: &[RenderBlock]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|block| match block {
            RenderBlock::System { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The incident, shape for shape: Opus 5.5, a `cyber` decline the provider
/// routes nowhere, a conversation short enough that nothing can be folded, and
/// the person's `continue`. Turn one is the request and the one same-model
/// retry, and is surfaced with the line the person saw. Turn two asks once and
/// says that the decline stands and what a bare `continue` will meet.
#[tokio::test]
async fn a_continue_after_the_persons_cyber_decline_asks_once_and_says_so() {
    let _catalog = the_persons_catalog();
    let client = Arc::new(Declining {
        calls: AtomicUsize::new(0),
        category: "cyber",
    });
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-opus-5-5");
    runtime.set_async_api_client(client.clone());

    let (iterations, blocks, first_took) = turn(&mut runtime, "look for the old key folder").await;
    let first = client.calls.load(Ordering::SeqCst);
    assert_eq!(first, 2, "turn one: the request and the same-model retry");
    assert_eq!(iterations, 2);
    let lines = system_lines(&blocks);
    assert!(
        lines.iter().any(|line| line.contains("giving up on automatic retries")),
        "the ordinary notice: {lines:?}"
    );
    assert!(
        lines.contains(&core_types::retry_signal::refusal_stands_notice("cyber")),
        "and the line the person saw — the provider routes `cyber` to no other model: {lines:?}"
    );

    let (iterations, blocks, took) = turn(&mut runtime, "continue").await;
    let second = client.calls.load(Ordering::SeqCst) - first;
    eprintln!(
        "t-15890 measured | the incident shape (Opus 5.5, cyber, no route, short conversation) | \
         turn one {first} requests in {:.2}s | the continue turn {second} requests in {:.2}s at {:?} a request",
        first_took.as_secs_f64(),
        took.as_secs_f64(),
        REQUEST_LATENCY,
    );
    assert_eq!(second, 1, "turn two asks once");
    assert_eq!(iterations, 1);
    let lines = system_lines(&blocks);
    assert!(
        lines.contains(&core_types::retry_signal::REFUSAL_STANDING_NOTICE.to_string()),
        "and says what a bare continue will meet: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("giving up on automatic retries")),
        "in place of the ordinary notice: {lines:?}"
    );
    let last = runtime.session().messages.last().expect("the surfaced notice");
    assert!(
        matches!(
            last.blocks.first(),
            Some(runtime::ContentBlock::Text { text })
                if text == core_types::retry_signal::REFUSAL_STANDING_NOTICE
        ),
        "the conversation records the same words: {last:?}"
    );
}

/// t-15890's path covers the Sonnet lineup too (t-17474). A Sonnet 5.5
/// `reasoning_extraction` decline is routed nowhere — the `sonnet` row used to
/// declare no routes, so the ladder offered Opus 5.5, behind the same
/// classifier, and asked the person whether to continue there. Now the turn
/// makes the request and the one same-model retry, says the decline stands,
/// and asks nothing; the person's `continue` asks once and hears what a bare
/// `continue` will meet, as on Opus.
#[tokio::test]
async fn a_continue_after_a_sonnet_decline_the_provider_routes_nowhere_asks_once_and_says_so() {
    let _catalog = the_persons_catalog();
    QUESTIONS_ASKED.store(0, Ordering::SeqCst);
    let client = Arc::new(Declining {
        calls: AtomicUsize::new(0),
        category: "reasoning_extraction",
    });
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-sonnet-5-5");
    runtime.set_attendance(runtime::Attendance::Attended);
    runtime.set_classifier_fallback(runtime::ClassifierFallback::Ask);
    runtime.set_async_api_client(client.clone());

    let (iterations, blocks, first_took) = turn(&mut runtime, "say hello").await;
    let first = client.calls.load(Ordering::SeqCst);
    assert_eq!(first, 2, "turn one: the request and the same-model retry");
    assert_eq!(iterations, 2);
    assert_eq!(
        QUESTIONS_ASKED.load(Ordering::SeqCst),
        0,
        "no question about leaving Sonnet 5.5 for a model behind the same classifier"
    );
    let lines = system_lines(&blocks);
    assert!(
        lines.contains(&core_types::retry_signal::refusal_stands_notice("reasoning_extraction")),
        "the provider routes `reasoning_extraction` to no other model, and the turn says so: {lines:?}"
    );

    let (iterations, blocks, took) = turn(&mut runtime, "continue").await;
    let second = client.calls.load(Ordering::SeqCst) - first;
    eprintln!(
        "t-17474 measured | Sonnet 5.5, reasoning_extraction, no route | turn one {first} requests, \
         0 questions in {:.2}s | the continue turn {second} requests in {:.2}s at {:?} a request",
        first_took.as_secs_f64(),
        took.as_secs_f64(),
        REQUEST_LATENCY,
    );
    assert_eq!(second, 1, "turn two asks once");
    assert_eq!(iterations, 1);
    assert_eq!(QUESTIONS_ASKED.load(Ordering::SeqCst), 0);
    let lines = system_lines(&blocks);
    assert!(
        lines.contains(&core_types::retry_signal::REFUSAL_STANDING_NOTICE.to_string()),
        "and says what a bare continue will meet: {lines:?}"
    );
}
