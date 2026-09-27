//! The turn-end gate and the session's background work (t-11354).
//!
//! On 2026-09-27 a main turn tried to end with "두 에이전트가 백그라운드에서
//! 돌고 있고, 결과는 끝나는 대로 이 대화로 들어옵니다 … push하겠습니다" — the
//! ending the `Agent` tool's own note asks for ("when nothing is left but
//! waiting, end your turn"). The gate read "하겠습니다" as a broken promise and
//! sent it back to "do that work now with tool calls", so the model polled the
//! agents' state files with sleep loops for five hours and the person could
//! not talk to it. A person at the keyboard whose session has background work
//! out may end the turn on what those results will bring; everything else the
//! gate does stays as it was — these tests pin both halves.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use runtime::message_stream::types::BlockId;
use runtime::message_stream::RenderBlock;
use runtime::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError, PermissionPrompter,
    PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::{MessageRole, Session};
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, Attendance, BackgroundWorkProbe,
    ContentBlock, ConversationRuntime, PermissionMode, PermissionPolicy, RuntimeError,
    StaticToolExecutor, TurnSummary, DEFAULT_STREAMING_CHANNEL_CAPACITY,
};
use tokio::sync::mpsc;

/// The reply the gate sent back on 2026-09-27, in the shape it had: the work
/// handed to two background agents, the last paragraph a promise about what
/// happens when their results land.
const WAITING_ON_AGENTS_REPLY: &str = "아직 끝나지 않았습니다. 지금 두 에이전트가 백그라운드에서 돌고 있고, 각 결과는 끝나는 대로 이 대화로 들어옵니다.\n\n새 실패 없이 전체 검증을 통과해야 push하겠습니다.";

/// What the scripted model says once the gate has re-prompted it.
const AFTER_THE_GATE_REPLY: &str = "Both agents are still running; nothing else is left to do now.";

const GATE_MARKER: &str = "[zo:turn-end-gate]";

/// The quota tests of this crate read `ZO_SHARED_RATE_COORD`; every test here
/// pins it off, the way `conversation_streaming_seam.rs` does, so a developer's
/// live shared-quota observation never reaches a scripted turn.
static RATE_COORD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// Answers with [`WAITING_ON_AGENTS_REPLY`] until a request carries the gate's
/// reminder, then with [`AFTER_THE_GATE_REPLY`]; counts the calls.
struct WaitingOnAgentsModel {
    calls: Arc<AtomicUsize>,
}

impl AsyncApiClient for WaitingOnAgentsModel {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let reprompted = request.messages.iter().any(|message| {
                message.blocks.iter().any(|block| {
                    matches!(block, ContentBlock::Text { text } if text.contains(GATE_MARKER))
                })
            });
            let text = if reprompted {
                AFTER_THE_GATE_REPLY
            } else {
                WAITING_ON_AGENTS_REPLY
            };
            render_tx
                .send(RenderBlock::TextDelta {
                    id: text_block_id,
                    text: text.to_string(),
                    done: true,
                })
                .await
                .map_err(|_| RuntimeError::new("channel closed"))?;
            Ok(vec![
                AssistantEvent::TextDelta(text.to_string()),
                AssistantEvent::MessageStop,
            ])
        })
    }
}

/// One streaming turn on the scripted model, with the host's attendance and
/// background-work probe as given. Returns the summary, the runtime (for its
/// transcript) and how many requests the model saw.
async fn turn_with(
    attendance: Option<Attendance>,
    probe: Option<BackgroundWorkProbe>,
) -> (
    TurnSummary,
    ConversationRuntime<ExplodingSyncApi, StaticToolExecutor>,
    usize,
) {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ExplodingSyncApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    let calls = Arc::new(AtomicUsize::new(0));
    runtime.set_async_api_client(Arc::new(WaitingOnAgentsModel {
        calls: Arc::clone(&calls),
    }));
    if let Some(attendance) = attendance {
        runtime.set_attendance(attendance);
    }
    runtime.set_background_work_probe(probe);

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("완벽하게 마무리 시켜 두작업다", tx, prompter)
        .await
        .expect("the turn runs");
    drain.await.expect("drain");
    let calls = calls.load(Ordering::SeqCst);
    (summary, runtime, calls)
}

fn gate_reminders(runtime: &ConversationRuntime<ExplodingSyncApi, StaticToolExecutor>) -> usize {
    runtime
        .session()
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .flat_map(|message| message.blocks.iter())
        .filter(|block| matches!(block, ContentBlock::Text { text } if text.starts_with(GATE_MARKER)))
        .count()
}

fn out(count: usize) -> BackgroundWorkProbe {
    Arc::new(move || count)
}

/// The live case: a person at the keyboard, two agents out. The reply ends
/// the turn as it is — one request, no reminder — so the person can talk,
/// and the agents' results come back as their own messages.
#[tokio::test]
async fn an_attended_turn_whose_work_is_out_in_the_background_ends_on_what_it_awaits() {
    let _env = hermetic_env();
    let (summary, runtime, calls) = turn_with(Some(Attendance::Attended), Some(out(2))).await;

    assert_eq!(gate_reminders(&runtime), 0, "the gate sent the turn back");
    assert_eq!(calls, 1, "one request: the turn ended on its own reply");
    assert_eq!(summary.assistant_messages.len(), 1);
    assert_eq!(
        runtime::final_assistant_text(&summary),
        WAITING_ON_AGENTS_REPLY,
        "the turn ends on the reply that names what it awaits"
    );
}

/// With nothing out, the same promise is the one the gate exists for: it is
/// sent back once, exactly as before.
#[tokio::test]
async fn with_no_background_work_out_the_promise_is_sent_back_as_before() {
    let _env = hermetic_env();
    let (summary, runtime, calls) = turn_with(Some(Attendance::Attended), Some(out(0))).await;

    assert_eq!(gate_reminders(&runtime), 1);
    assert_eq!(calls, 2);
    assert_eq!(
        runtime::final_assistant_text(&summary),
        AFTER_THE_GATE_REPLY
    );
}

/// Nobody at the keyboard — a headless run, a turn the window drives — keeps
/// the gate whole, background work or not: there is nobody for an early end
/// to free, and nothing brings the result back as a message there.
#[tokio::test]
async fn an_unattended_turn_is_gated_whatever_is_out() {
    let _env = hermetic_env();
    let (_, runtime, calls) = turn_with(Some(Attendance::Unattended), Some(out(2))).await;

    assert_eq!(gate_reminders(&runtime), 1);
    assert_eq!(calls, 2);
}

/// A host that installs no probe brings nothing back as a message: the gate
/// reads that as "nothing out" and stays as it was.
#[tokio::test]
async fn a_host_without_a_probe_keeps_the_gate_as_it_was() {
    let _env = hermetic_env();
    let (_, runtime, calls) = turn_with(Some(Attendance::Attended), None).await;

    assert_eq!(gate_reminders(&runtime), 1);
    assert_eq!(calls, 2);
}
