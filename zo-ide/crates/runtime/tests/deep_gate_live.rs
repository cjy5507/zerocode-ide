//! Deep-lane gate — live orchestration tests.
//!
//! Drives [`ConversationRuntime::run_deep_turn_streaming`] over a scripted
//! `ApiClient` (replayed through the streaming path) to assert the plan →
//! implement → verify → decide loop wires the `decision-core` policy correctly:
//! accept on a clean verify, retry on a strict-JSON rejection, give up when out
//! of attempts, re-plan when the plan is structurally invalid, and — the
//! security contract — block edits during the read-only PLAN phase while still
//! allowing them during IMPLEMENT.
//!
//! The objective check command is `None` in the decision-path tests so they are
//! hermetic (no subprocess); the green/red gate is exercised by the unit tests
//! over `interpret_green` in `conversation/deep_gate.rs`.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use runtime::message_stream::RenderBlock;
use runtime::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError, PermissionPrompter,
    PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::Session;
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, DeepGateConfig, DeepMode,
    PermissionMode, PermissionPolicy, RuntimeError, StaticToolExecutor,
    DEFAULT_STREAMING_CHANNEL_CAPACITY,
};
use tokio::sync::mpsc;

const VALID_PLAN: &str = "## Target files\n- a.rs: add the field\n\
     ## Invariants\n- public API unchanged\n\
     ## Expected tests\n- cargo test\n\
     ## Risks\n- serializer drift";
const INVALID_PLAN: &str = "I'll just change a.rs and call it done.";
const ACCEPT_JSON: &str = r#"{"accepted": true, "issues": []}"#;
const REJECT_JSON: &str = r#"{"accepted": false, "issues": ["missing null check"]}"#;

/// API client that replays a fixed script, one entry per model turn. Shares a
/// call counter so a test can assert how many phase sub-turns ran.
struct ScriptedApi {
    script: Vec<Vec<AssistantEvent>>,
    calls: Arc<AtomicUsize>,
}

impl ApiClient for ScriptedApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.script.get(index).cloned().unwrap_or_else(|| {
            // Defensive default: a clean empty stop, so an over-run loop ends
            // instead of panicking on an out-of-range script.
            vec![
                AssistantEvent::TextDelta(String::new()),
                AssistantEvent::MessageStop,
            ]
        }))
    }
}

/// A text-only model turn: emit `body`, then stop.
fn text_turn(body: &str) -> Vec<AssistantEvent> {
    vec![
        AssistantEvent::TextDelta(body.to_string()),
        AssistantEvent::MessageStop,
    ]
}

/// A turn that calls one tool then stops (the runtime executes the tool and
/// loops to the next scripted turn for the final text).
fn tool_turn(name: &str, input: &str) -> Vec<AssistantEvent> {
    vec![
        AssistantEvent::TextDelta("working".to_string()),
        AssistantEvent::ToolUse {
            id: "t1".to_string(),
            name: name.to_string(),
            input: input.to_string(),
        },
        AssistantEvent::MessageStop,
    ]
}

/// Prompter that always denies — used so a `ReadOnly` escalation during PLAN is
/// refused. Decision-path tests never trigger a prompt (their phases are
/// text-only).
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

/// Build a deep-gate runtime over `script` with `max_attempts` and no objective
/// check command. Returns the runtime and the shared call counter.
fn deep_runtime(
    script: Vec<Vec<AssistantEvent>>,
    max_attempts: u32,
) -> (
    ConversationRuntime<ScriptedApi, StaticToolExecutor>,
    Arc<AtomicUsize>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedApi {
            script,
            calls: Arc::clone(&calls),
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_deep_gate(Some(DeepGateConfig {
        mode: DeepMode::PlanFirst,
        check_command: None,
        max_attempts,
    }));
    (runtime, calls)
}

/// Drive a deep turn to completion, draining the render channel concurrently.
async fn run_deep(
    runtime: &mut ConversationRuntime<ScriptedApi, StaticToolExecutor>,
) -> (runtime::TurnSummary, runtime::DeepOutcome) {
    let (tx, mut rx) = mpsc::channel::<RenderBlock>(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let result = runtime
        .run_deep_turn_streaming("do the task", Vec::new(), tx, prompter)
        .await
        .expect("deep turn");
    drain.await.expect("drain");
    result
}

#[tokio::test]
async fn deep_accepts_on_first_clean_verify() {
    let (mut runtime, calls) = deep_runtime(
        vec![
            text_turn(VALID_PLAN),               // PLAN
            text_turn("implemented the change"), // EXEC
            text_turn(ACCEPT_JSON),              // VERIFY
        ],
        3,
    );
    let (_summary, outcome) = run_deep(&mut runtime).await;
    assert_eq!(outcome.decision.as_str(), "accept");
    assert_eq!(outcome.attempts, 1);
    assert!(outcome.plan_valid);
    assert_eq!(calls.load(Ordering::SeqCst), 3, "plan + exec + verify");
}

#[tokio::test]
async fn deep_retries_after_rejection_then_accepts() {
    let (mut runtime, calls) = deep_runtime(
        vec![
            text_turn(VALID_PLAN),  // PLAN
            text_turn("attempt 1"), // EXEC 1
            text_turn(REJECT_JSON), // VERIFY 1 → reject → retry
            text_turn("attempt 2"), // EXEC 2
            text_turn(ACCEPT_JSON), // VERIFY 2 → accept
        ],
        3,
    );
    let (_summary, outcome) = run_deep(&mut runtime).await;
    assert_eq!(outcome.decision.as_str(), "accept");
    assert_eq!(outcome.attempts, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn deep_gives_up_when_out_of_attempts() {
    let (mut runtime, _calls) = deep_runtime(
        vec![
            text_turn(VALID_PLAN),  // PLAN
            text_turn("attempt 1"), // EXEC 1
            text_turn(REJECT_JSON), // VERIFY 1 → reject → retry
            text_turn("attempt 2"), // EXEC 2
            text_turn(REJECT_JSON), // VERIFY 2 → reject → out of attempts
        ],
        2,
    );
    let (_summary, outcome) = run_deep(&mut runtime).await;
    assert_eq!(outcome.decision.as_str(), "give_up");
    assert_eq!(outcome.attempts, 2);
}

#[tokio::test]
async fn deep_replans_when_plan_is_structurally_invalid() {
    let (mut runtime, calls) = deep_runtime(
        vec![
            text_turn(INVALID_PLAN),  // PLAN 1 → invalid → re-plan
            text_turn(VALID_PLAN),    // PLAN 2 → valid
            text_turn("implemented"), // EXEC
            text_turn(ACCEPT_JSON),   // VERIFY → accept
        ],
        3,
    );
    let (_summary, outcome) = run_deep(&mut runtime).await;
    assert_eq!(outcome.decision.as_str(), "accept");
    assert!(outcome.plan_valid, "re-plan must have reached a valid plan");
    assert_eq!(outcome.attempts, 1);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "two plan turns + exec + verify"
    );
}

#[tokio::test]
async fn deep_plan_phase_is_read_only_but_implement_can_write() {
    // write_file requires WorkspaceWrite; the base mode is WorkspaceWrite so a
    // write is normally allowed. The PLAN phase downgrades to ReadOnly, so the
    // plan turn's write must be refused while the EXEC turn's write succeeds.
    let writes = Arc::new(AtomicUsize::new(0));
    let writes_in_handler = Arc::clone(&writes);
    let calls = Arc::new(AtomicUsize::new(0));

    let script = vec![
        tool_turn("write_file", "{\"path\":\"a.rs\",\"content\":\"x\"}"), // PLAN iter1: blocked
        text_turn(VALID_PLAN),                                            // PLAN iter2: valid
        tool_turn("write_file", "{\"path\":\"a.rs\",\"content\":\"y\"}"), // EXEC iter1: allowed
        text_turn("done"),                                                // EXEC iter2
        text_turn(ACCEPT_JSON),                                           // VERIFY
    ];

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedApi {
            script,
            calls: Arc::clone(&calls),
        },
        StaticToolExecutor::new().register("write_file", move |_input| {
            writes_in_handler.fetch_add(1, Ordering::SeqCst);
            Ok("wrote".to_string())
        }),
        PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("write_file", PermissionMode::WorkspaceWrite),
        vec!["system".to_string()],
    );
    runtime.set_deep_gate(Some(DeepGateConfig {
        mode: DeepMode::PlanFirst,
        check_command: None,
        max_attempts: 1,
    }));

    let (_summary, outcome) = run_deep(&mut runtime).await;

    assert_eq!(outcome.decision.as_str(), "accept");
    assert_eq!(
        writes.load(Ordering::SeqCst),
        1,
        "write must be blocked in the ReadOnly PLAN phase and allowed only in EXEC"
    );
}

// ── Reactive auto-verify (DeepMode::Reactive, the default) ───────────────────

/// A reactive runtime: a `write_file` tool the model can call, and the gate in
/// reactive mode with no objective check command.
fn reactive_runtime(
    script: Vec<Vec<AssistantEvent>>,
    max_attempts: u32,
) -> (
    ConversationRuntime<ScriptedApi, StaticToolExecutor>,
    Arc<AtomicUsize>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedApi {
            script,
            calls: Arc::clone(&calls),
        },
        StaticToolExecutor::new().register("write_file", |_input| Ok("wrote".to_string())),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_deep_gate(Some(DeepGateConfig {
        mode: DeepMode::Reactive,
        check_command: None,
        max_attempts,
    }));
    (runtime, calls)
}

async fn run_auto(
    runtime: &mut ConversationRuntime<ScriptedApi, StaticToolExecutor>,
) -> (runtime::TurnSummary, runtime::DeepOutcome) {
    let (tx, mut rx) = mpsc::channel::<RenderBlock>(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let result = runtime
        .run_auto_turn_streaming("improve it", Vec::new(), tx, prompter)
        .await
        .expect("auto turn");
    drain.await.expect("drain");
    result
}

#[tokio::test]
async fn auto_skips_verification_when_no_edits() {
    // A pure analysis/chat turn (no tool calls) must NOT trigger a verify pass.
    let (mut runtime, calls) = reactive_runtime(vec![text_turn("here is my analysis")], 2);
    let (_summary, outcome) = run_auto(&mut runtime).await;
    assert_eq!(outcome.decision.as_str(), "accept");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no edits ⇒ exactly one turn, no verification overhead"
    );
}

#[tokio::test]
async fn auto_verifies_and_accepts_after_an_edit() {
    let (mut runtime, calls) = reactive_runtime(
        vec![
            tool_turn("write_file", "{\"path\":\"a.rs\",\"content\":\"x\"}"), // edit iter1
            text_turn("done"),                                                // edit iter2
            text_turn(ACCEPT_JSON),                                           // VERIFY
        ],
        2,
    );
    let (_summary, outcome) = run_auto(&mut runtime).await;
    assert_eq!(outcome.decision.as_str(), "accept");
    assert_eq!(outcome.attempts, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 3, "edit(2) + verify(1)");
}

#[tokio::test]
async fn auto_retries_after_rejection_then_accepts() {
    let (mut runtime, _calls) = reactive_runtime(
        vec![
            tool_turn("write_file", "{\"path\":\"a.rs\",\"content\":\"x\"}"), // attempt1 edit
            text_turn("done"),
            text_turn(REJECT_JSON), // VERIFY 1 → reject → retry
            tool_turn("write_file", "{\"path\":\"a.rs\",\"content\":\"y\"}"), // attempt2 edit
            text_turn("fixed"),
            text_turn(ACCEPT_JSON), // VERIFY 2 → accept
        ],
        2,
    );
    let (_summary, outcome) = run_auto(&mut runtime).await;
    assert_eq!(outcome.decision.as_str(), "accept");
    assert_eq!(outcome.attempts, 2);
}

/// Regression (TUI freeze): the reactive gate's objective check command must not
/// starve the host's `select!` event loop. The host (`drive_turn`) polls the
/// turn future on the same task as its key/mouse/spinner arms; the objective
/// check runs the project test command through the blocking `execute_bash`
/// chokepoint, which can take minutes. Before the fix that check ran via
/// `block_in_place`, suspending the whole task — the user saw the spinner freeze
/// (e.g. "Drafting response · 12m") with input and mouse wheel dead until the
/// command returned. The fix offloads the check to `spawn_blocking` so the await
/// yields and the loop stays live.
///
/// We reproduce the host loop faithfully: pin the reactive turn future (whose
/// edit triggers a deliberately slow `sleep 1` objective check) and race it
/// against a 20 ms timer in a `select!`, exactly like `drive_turn`'s render
/// tick. A live loop services the timer many times while the check runs; the
/// pre-fix blocking check would let zero ticks through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reactive_check_does_not_starve_the_render_loop() {
    use std::time::Duration;

    let calls = Arc::new(AtomicUsize::new(0));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedApi {
            script: vec![
                tool_turn("write_file", "{\"path\":\"a.rs\",\"content\":\"x\"}"), // edit iter1
                text_turn("done"),                                                // edit iter2
                text_turn(ACCEPT_JSON),                                           // VERIFY
            ],
            calls: Arc::clone(&calls),
        },
        StaticToolExecutor::new().register("write_file", |_input| Ok("wrote".to_string())),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    // A deliberately slow objective check so the test has a wide, robust window
    // to observe loop liveness. `sleep 1` exits 0 (green), so the verify gate
    // still accepts and the turn completes normally.
    runtime.set_deep_gate(Some(DeepGateConfig {
        mode: DeepMode::Reactive,
        check_command: Some("sleep 1".to_string()),
        max_attempts: 1,
    }));

    let (tx, mut rx) = mpsc::channel::<RenderBlock>(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);

    let turn = runtime.run_auto_turn_streaming("improve it", Vec::new(), tx, prompter);
    tokio::pin!(turn);

    let mut ticks = 0u32;
    let mut interval = tokio::time::interval(Duration::from_millis(20));
    interval.tick().await; // skip the immediate first tick

    let outcome = loop {
        tokio::select! {
            result = &mut turn => break result.expect("auto turn"),
            _ = interval.tick() => ticks += 1,
        }
    };
    drain.await.expect("drain");

    assert_eq!(outcome.1.decision.as_str(), "accept");
    // The `sleep 1` check alone gives ~50 ticks of headroom; require a generous
    // floor so the test is robust on a busy CI box. A starved loop (the pre-fix
    // `block_in_place`) would land 0 ticks during the blocking check.
    assert!(
        ticks >= 5,
        "event loop was starved while the objective check ran: only {ticks} timer ticks fired"
    );
}

/// API client that records each request's `effort_override` (in call order)
/// while replaying a fixed script — used to assert the deep-gate escalates
/// reasoning effort on a retry and clears it for the following verify turn.
struct RecordingApi {
    script: Vec<Vec<AssistantEvent>>,
    calls: Arc<AtomicUsize>,
    efforts: Arc<Mutex<Vec<Option<u32>>>>,
}

impl ApiClient for RecordingApi {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.efforts
            .lock()
            .expect("efforts lock")
            .push(request.effort_override);
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.script.get(index).cloned().unwrap_or_else(|| {
            vec![
                AssistantEvent::TextDelta(String::new()),
                AssistantEvent::MessageStop,
            ]
        }))
    }
}

#[tokio::test]
async fn deep_escalates_effort_on_retry_and_clears_it_for_verify() {
    // Script: PLAN, EXEC1, VERIFY1(reject)→retry, EXEC2, VERIFY2(accept).
    let efforts = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        RecordingApi {
            script: vec![
                text_turn(VALID_PLAN),  // 0 PLAN
                text_turn("attempt 1"), // 1 EXEC 1
                text_turn(REJECT_JSON), // 2 VERIFY 1 → reject → retry
                text_turn("attempt 2"), // 3 EXEC 2 (escalated)
                text_turn(ACCEPT_JSON), // 4 VERIFY 2 → accept
            ],
            calls: Arc::clone(&calls),
            efforts: Arc::clone(&efforts),
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_deep_gate(Some(DeepGateConfig {
        mode: DeepMode::PlanFirst,
        check_command: None,
        max_attempts: 3,
    }));

    let (tx, mut rx) = mpsc::channel::<RenderBlock>(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let (_summary, outcome) = runtime
        .run_deep_turn_streaming("do the task", Vec::new(), tx, prompter)
        .await
        .expect("deep turn");
    drain.await.expect("drain");

    assert_eq!(outcome.decision.as_str(), "accept");
    assert_eq!(outcome.attempts, 2);

    let recorded = efforts.lock().expect("efforts lock").clone();
    assert_eq!(
        recorded.len(),
        5,
        "plan + exec1 + verify1 + exec2 + verify2"
    );
    // First attempt and all non-retry turns run at the configured effort (None).
    assert_eq!(recorded[0], None, "PLAN: no escalation");
    assert_eq!(recorded[1], None, "EXEC attempt 1: no escalation");
    assert_eq!(recorded[2], None, "VERIFY 1: no escalation");
    // The retry EXEC is escalated to the Xhigh floor (16_000).
    assert_eq!(recorded[3], Some(16_000), "EXEC retry: escalated to xhigh");
    // The escalation is cleared before the following read-only VERIFY.
    assert_eq!(recorded[4], None, "VERIFY after retry: escalation cleared");
}

#[tokio::test]
async fn auto_escalates_effort_on_retry_and_clears_it_for_verify() {
    // The reactive default must mirror the plan-first escalation: a retry runs
    // the EXEC leg at the Xhigh floor, but the read-only VERIFY leg (and the
    // first, un-escalated attempt) stay at the configured effort. Before the fix
    // the reactive path re-ran retries at the same failed effort.
    //
    // Each EXEC leg is two model iterations (the edit's tool turn + the final
    // text turn), and the escalation floor is set once for the whole leg, so all
    // iterations of the escalated attempt record the floor.
    let efforts = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        RecordingApi {
            script: vec![
                tool_turn("write_file", "{\"path\":\"a.rs\",\"content\":\"x\"}"), // 0 EXEC1 edit
                text_turn("done"),                                                // 1 EXEC1 text
                text_turn(REJECT_JSON), // 2 VERIFY 1 → reject → retry
                tool_turn("write_file", "{\"path\":\"a.rs\",\"content\":\"y\"}"), // 3 EXEC2 edit
                text_turn("fixed"),                                               // 4 EXEC2 text
                text_turn(ACCEPT_JSON), // 5 VERIFY 2 → accept
            ],
            calls: Arc::clone(&calls),
            efforts: Arc::clone(&efforts),
        },
        StaticToolExecutor::new().register("write_file", |_input| Ok("wrote".to_string())),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_deep_gate(Some(DeepGateConfig {
        mode: DeepMode::Reactive,
        check_command: None,
        max_attempts: 2,
    }));

    let (tx, mut rx) = mpsc::channel::<RenderBlock>(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let (_summary, outcome) = runtime
        .run_auto_turn_streaming("improve it", Vec::new(), tx, prompter)
        .await
        .expect("auto turn");
    drain.await.expect("drain");

    assert_eq!(outcome.decision.as_str(), "accept");
    assert_eq!(outcome.attempts, 2);

    let recorded = efforts.lock().expect("efforts lock").clone();
    assert_eq!(
        recorded.len(),
        6,
        "exec1(edit+text) + verify1 + exec2(edit+text) + verify2"
    );
    // Attempt 1 (both EXEC iterations) and VERIFY 1 run at the configured effort.
    assert_eq!(recorded[0], None, "EXEC attempt 1 edit: no escalation");
    assert_eq!(recorded[1], None, "EXEC attempt 1 text: no escalation");
    assert_eq!(recorded[2], None, "VERIFY 1: no escalation");
    // The retry EXEC leg is escalated to the Xhigh floor across both iterations.
    assert_eq!(recorded[3], Some(16_000), "EXEC retry edit: escalated to xhigh");
    assert_eq!(recorded[4], Some(16_000), "EXEC retry text: escalated to xhigh");
    // The escalation is cleared before the following read-only VERIFY.
    assert_eq!(recorded[5], None, "VERIFY after retry: escalation cleared");
}
