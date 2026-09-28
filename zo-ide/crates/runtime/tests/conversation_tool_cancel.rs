//! Esc-once semantics at the runtime level: cancel the *tool*, keep the *turn*.
//!
//! The claim under test is not "the tool stops" — it is the pair:
//!   1. the cancelled call settles a real `tool_result` in stored history (not
//!      a view-only seal), so the model actually reads the cancellation, and
//!   2. the turn survives it and runs the next leg.
//!
//! Both halves matter. Settling without continuing is the old turn-kill wearing
//! a new name; continuing without settling leaves an orphan `tool_use` that only
//! `tool_consistent_messages` can paper over per request.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use runtime::message_stream::{RenderBlock, ToolCallStatus};
use runtime::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError, PermissionPrompter,
    PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::{MessageRole, Session};
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ConcurrentDispatchFn, ContentBlock, ConversationRuntime,
    HookAbortSignal, PermissionMode, PermissionPolicy, RuntimeError, StaticToolExecutor,
    StreamingTurnError, ToolCancelSignal, CANCELLED_TOOL_RESULT,
    DEFAULT_STREAMING_CHANNEL_CAPACITY, STEERING_ECHO_PREFIX,
};
use tokio::sync::mpsc;

/// How long the fake wedged tool would run if nothing cancelled it.
///
/// Kept short only because the suite has to end: the detached blocking task
/// genuinely outlives the turn (that is the documented limitation), and tokio's
/// shutdown waits for it. The assertions below prove the *turn* did not wait.
const WEDGED_TOOL_RUNTIME: Duration = Duration::from_secs(3);

/// One tool call, then prose. The second stream only happens if the turn
/// survived the cancellation, so `calls == 2` is the "turn continued" assertion.
struct OneToolThenText {
    calls: Arc<AtomicUsize>,
    tool_name: &'static str,
}

impl ApiClient for OneToolThenText {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == 1 {
            Ok(vec![
                AssistantEvent::ToolUse {
                    id: "tool-1".to_string(),
                    name: self.tool_name.to_string(),
                    input: "{}".to_string(),
                },
                AssistantEvent::MessageStop,
            ])
        } else {
            Ok(vec![
                AssistantEvent::TextDelta("recovered".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
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

fn cancelled_tool_results(runtime_session: &Session) -> Vec<(String, String, bool)> {
    runtime_session
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                output,
                is_error,
                ..
            } => Some((tool_use_id.clone(), output.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

// ── Harness attestation ─────────────────────────────────────────────────────
//
// Esc-once is at its most invisible when it works: what the user sees is a turn
// that kept going, which is exactly what they see when nothing was cancelled.
// The attest ledger is the only place a firing leaves a durable mark, and
// `/smart doctor` and `/refine` read a never-firing feature as a dead one.

/// The ledger is PROCESS-wide by design and both tests below share it, so an
/// exact-delta assertion would be a coin flip under cargo's default
/// parallelism. Both tests take this lock, which makes each delta its own work.
///
/// An ASYNC mutex on purpose: the guard is held across the turn's awaits,
/// because the turn IS the measured region. It also has no poisoning, so one
/// failing test cannot cascade into the other and hide which one broke.
static ATTEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Settled tool cancellations recorded in this process so far.
fn cancel_firings() -> u64 {
    telemetry::harness_attest_snapshot()
        .attestation(telemetry::HarnessFeature::ToolCancelSettled)
        .fired
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_running_tool_settles_a_tool_result_and_keeps_the_turn() {
    let _serial = ATTEST_SERIAL.lock().await;
    let firings_before = cancel_firings();
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatch_entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let entered = Arc::clone(&dispatch_entered);
    // A tool that would outlast the test: the cancel must be what ends the
    // wait, not the tool finishing on its own.
    let dispatch: ConcurrentDispatchFn = Arc::new(move |_name, _input| {
        entered.store(true, Ordering::SeqCst);
        std::thread::sleep(WEDGED_TOOL_RUNTIME);
        Ok("never observed".to_string())
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        OneToolThenText {
            calls: Arc::clone(&calls),
            tool_name: "Bash",
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_concurrent_dispatch(dispatch);
    let cancel = runtime.tool_cancel_signal();

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let blocks = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&blocks);
    let drain = tokio::spawn(async move {
        while let Some(block) = rx.recv().await {
            sink.lock().expect("block sink").push(block);
        }
    });

    // Fire the cancel once the tool is genuinely executing — the epoch only
    // catches dispatches already in flight, by design.
    let canceller = tokio::spawn(async move {
        while !dispatch_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel_running_tools();
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let started = std::time::Instant::now();
    let summary = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.run_turn_streaming("go", tx, prompter),
    )
    .await
    .expect("Esc-once must not leave the turn waiting on the cancelled tool")
    .expect("the turn continues past a cancelled tool");
    let turn_wall = started.elapsed();
    canceller.await.expect("canceller");
    drain.await.expect("drain");

    // 0. The turn stopped *waiting* — it did not ride the wedged tool out.
    assert!(
        turn_wall < WEDGED_TOOL_RUNTIME,
        "the turn must resume at the cancel, not when the tool eventually returns          (waited {turn_wall:?} of a {WEDGED_TOOL_RUNTIME:?} tool)"
    );

    // 1. The cancellation is a real, stored tool_result — not a view-only seal.
    let results = cancelled_tool_results(runtime.session());
    assert_eq!(
        results,
        vec![(
            "tool-1".to_string(),
            CANCELLED_TOOL_RESULT.to_string(),
            true
        )],
        "the cancelled call must settle a stored error tool_result the model reads"
    );

    // 2. The turn survived: a second leg ran and produced prose.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the turn must continue from the tool-result boundary, not die with the tool"
    );
    assert!(
        summary.iterations >= 2,
        "expected a second iteration after the cancelled tool, got {}",
        summary.iterations
    );

    // 3. The card says "you stopped this", not "this failed".
    let blocks = blocks.lock().expect("block sink");
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::ToolCall {
                status: ToolCallStatus::Cancelled,
                ..
            }
        )),
        "a cancelled tool must repaint its card as Cancelled before the error result lands"
    );

    // 4. …and it left evidence that it ran. One cancelled tool, one firing —
    // both dispatch arms funnel through the same helper, so this also pins
    // that the parallel wave and the single dispatch cannot double-count.
    assert_eq!(
        cancel_firings() - firings_before,
        1,
        "a settled cancellation must attest exactly one firing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_requested_before_a_tool_starts_does_not_touch_it() {
    // The reset-race a boolean flag would lose: the user cancels tool A, the
    // model immediately calls tool B, and B must run normally.
    let _serial = ATTEST_SERIAL.lock().await;
    let firings_before = cancel_firings();
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatch: ConcurrentDispatchFn = Arc::new(|_name, _input| Ok("real output".to_string()));

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        OneToolThenText {
            calls: Arc::clone(&calls),
            tool_name: "Read",
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_concurrent_dispatch(dispatch);
    // Cancel BEFORE the turn starts: no dispatch is in flight, so nothing is
    // armed and the epoch bump must be inert.
    runtime.tool_cancel_signal().cancel_running_tools();

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime
        .run_turn_streaming("go", tx, prompter)
        .await
        .expect("turn completes");
    drain.await.expect("drain");

    let results = cancelled_tool_results(runtime.session());
    assert_eq!(results.len(), 1, "one tool, one result");
    assert_eq!(
        results[0].1, "real output",
        "a stale cancel must not settle over a tool that started after it"
    );
    assert!(!results[0].2, "an uncancelled tool result is not an error");

    // The negative half: an inert cancel must leave the ledger alone. A counter
    // that ticks on a stale keypress would report the feature as alive on a
    // build where it had stopped cancelling anything.
    assert_eq!(
        cancel_firings(),
        firings_before,
        "a cancel that touched no dispatch must attest no firing"
    );
}

/// The **host's** path, not the test's: a signal created outside the runtime and
/// installed with [`ConversationRuntime::set_tool_cancel_signal`] must cancel a
/// tool that is already in flight.
///
/// The two tests above reach the signal with the *getter* — they fire the
/// runtime's own default. That covers the dispatch bracket but says nothing
/// about the seam a TUI actually uses, and for a long time nothing used it: no
/// production caller ever installed a signal, so Esc pressed while a tool ran
/// raised two flags (`hook_abort`, `cancel`) that the dispatch `select!` does
/// not read, printed "interrupted", and let the turn keep going. On a wedged
/// MCP or bash call, forever.
///
/// So this pins the installed handle end to end: the epoch the host holds is
/// the epoch the dispatch watches.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signal_installed_by_the_host_cancels_a_running_tool() {
    let _serial = ATTEST_SERIAL.lock().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatch_entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let entered = Arc::clone(&dispatch_entered);
    let dispatch: ConcurrentDispatchFn = Arc::new(move |_name, _input| {
        entered.store(true, Ordering::SeqCst);
        std::thread::sleep(WEDGED_TOOL_RUNTIME);
        Ok("never observed".to_string())
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        OneToolThenText {
            calls: Arc::clone(&calls),
            tool_name: "Bash",
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_concurrent_dispatch(dispatch);

    // The host owns it first and hands the runtime a clone — the direction the
    // session host installs it, and the reason it survives a runtime rebuild.
    let host_signal = ToolCancelSignal::new();
    runtime.set_tool_cancel_signal(host_signal.clone());

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let canceller = tokio::spawn(async move {
        while !dispatch_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Exactly what `TurnScaffold::cancel_turn` does after raising its flags.
        host_signal.cancel_running_tools();
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let started = std::time::Instant::now();
    let summary = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.run_turn_streaming("go", tx, prompter),
    )
    .await
    .expect("an installed signal must free a turn parked on a wedged tool")
    .expect("the turn continues past a cancelled tool");
    let turn_wall = started.elapsed();
    canceller.await.expect("canceller");
    drain.await.expect("drain");

    assert!(
        turn_wall < WEDGED_TOOL_RUNTIME,
        "the installed signal must end the wait, not the tool returning \
         (waited {turn_wall:?} of a {WEDGED_TOOL_RUNTIME:?} tool)"
    );
    assert_eq!(
        cancelled_tool_results(runtime.session()),
        vec![(
            "tool-1".to_string(),
            CANCELLED_TOOL_RESULT.to_string(),
            true
        )],
        "the host's cancel must settle the same stored tool_result as the runtime's own"
    );
    assert!(
        summary.iterations >= 2,
        "the turn must survive the cancel, got {} iterations",
        summary.iterations
    );
}

/// The person's Esc raises the turn's stop and cancels the running tool in one
/// act (zo's `TurnScaffold::cancel_turn`, flags first): the tool cancel wakes
/// the turn at once, while the stop is a flag its host looks at every 25 ms.
/// The woken turn must end at that cancelled tool — before a steer typed during
/// the tool is folded into its result, and before another request goes out.
/// When it ran on instead, a quick machine let the turn outrun the host's look:
/// the steer rode the continuation, not the fresh turn Esc asked for
/// (`e2e_esc_with_pending_steer_resubmits_it_as_a_fresh_turn`, 5 of 15 once
/// zo's input loop stopped spinning, 0 of 15 before it did).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_cancelled_by_the_turns_stop_ends_the_turn_with_its_steer_unfolded() {
    let _serial = ATTEST_SERIAL.lock().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatch_entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let entered = Arc::clone(&dispatch_entered);
    let dispatch: ConcurrentDispatchFn = Arc::new(move |_name, _input| {
        entered.store(true, Ordering::SeqCst);
        std::thread::sleep(WEDGED_TOOL_RUNTIME);
        Ok("never observed".to_string())
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        OneToolThenText {
            calls: Arc::clone(&calls),
            tool_name: "Bash",
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_concurrent_dispatch(dispatch);
    let stop = HookAbortSignal::new();
    runtime.set_hook_abort_signal(stop.clone());
    let cancel = runtime.tool_cancel_signal();
    let steering = runtime.steering_handle();

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let blocks = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&blocks);
    let drain = tokio::spawn(async move {
        while let Some(block) = rx.recv().await {
            sink.lock().expect("block sink").push(block);
        }
    });

    // A steer typed while the tool runs, then Esc: the stop first, the tool
    // cancel second, as the host raises them.
    let typed = Arc::clone(&steering);
    let canceller = tokio::spawn(async move {
        while !dispatch_entered.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        typed
            .lock()
            .expect("steering queue")
            .push("typed during the tool".to_string());
        stop.abort();
        cancel.cancel_running_tools();
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.run_turn_streaming("go", tx, prompter),
    )
    .await
    .expect("a stopped turn must not wait on the cancelled tool");
    canceller.await.expect("canceller");
    drain.await.expect("drain");

    assert!(
        matches!(outcome, Err(StreamingTurnError::Cancelled)),
        "the stopped turn must end cancelled at the tool, not run another leg"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no request may go out after the stop"
    );
    assert_eq!(
        steering.lock().expect("steering queue").as_slice(),
        ["typed during the tool".to_string()],
        "the steer stays queued for the host to send as a fresh turn"
    );
    let blocks = blocks.lock().expect("block sink");
    assert!(
        !blocks.iter().any(|block| matches!(
            block,
            RenderBlock::System { text, .. } if text.starts_with(STEERING_ECHO_PREFIX)
        )),
        "the steer must not be folded into the stopped turn"
    );
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            RenderBlock::ToolCall {
                status: ToolCallStatus::Cancelled,
                ..
            }
        )),
        "the card still says the tool was stopped"
    );
    assert!(
        cancelled_tool_results(runtime.session()).is_empty(),
        "a stopped turn stores no \"the turn continues\" result; its tool_use is sealed as interrupted"
    );
}
