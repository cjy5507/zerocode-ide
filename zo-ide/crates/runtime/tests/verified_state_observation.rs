//! Verified-state observation, driven through the STREAMING dispatcher — the
//! path every live surface uses (TUI and all three headless output formats).
//!
//! The unit tests next to the ledger prove the rendering and the sidecar. What
//! only an end-to-end turn can prove is the pair the feature actually claims:
//!
//!   1. a green check observed in turn N reaches the WIRE at turn N+1 (inside
//!      `request.messages`, i.e. appended behind the new user message — not
//!      rewritten into the cached prefix), and
//!   2. a session that verified nothing sends bytes indistinguishable from a
//!      build without the feature.
//!
//! Both halves are silent failures otherwise: an observation that never reaches
//! the wire looks exactly like a session that had nothing to observe, which is
//! why the attest counters are asserted here too rather than trusted.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use runtime::message_stream::RenderBlock;
use runtime::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError, PermissionPrompter,
    PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::Session;
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ConcurrentDispatchFn, ContentBlock, ConversationRuntime,
    PermissionMode, PermissionPolicy, RuntimeError, StaticToolExecutor,
    DEFAULT_STREAMING_CHANNEL_CAPACITY, VERIFIED_STATE_REMINDER_PREFIX,
};
use tokio::sync::mpsc;

/// The attest ledger is process-wide, so an exact-delta assertion would be a
/// coin flip under cargo's default parallelism — including the NEGATIVE
/// assertion, which is the one that catches a feature firing when it must not.
/// Both tests take this lock, making each delta its own work.
static ATTEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Non-check tool batches run after the mutating edit in
/// [`an_edit_invalidates_the_line_and_retires_the_standing_block`]. Each batch
/// settles two messages (assistant + tool result), so this comfortably clears
/// the absorber's 12-message reminder dedupe window — the condition under which
/// a standing reminder set is re-anchored near the tail.
const MID_TURN_BATCHES: usize = 10;

fn observation_firings() -> u64 {
    telemetry::harness_attest_snapshot()
        .attestation(telemetry::HarnessFeature::VerifiedStateObservation)
        .fired
}

fn observation_declines() -> u64 {
    telemetry::harness_attest_snapshot()
        .attestation(telemetry::HarnessFeature::VerifiedStateObservation)
        .declined
        .get("no_green_checks")
        .copied()
        .unwrap_or_default()
}

/// A client that replays a script of responses and keeps every request it was
/// handed, so the test can assert on the exact payload that went out.
struct ScriptedClient {
    script: Arc<Mutex<VecDeque<Vec<AssistantEvent>>>>,
    requests: Arc<Mutex<Vec<ApiRequest>>>,
}

impl ApiClient for ScriptedClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.requests.lock().expect("requests").push(request);
        let next = self
            .script
            .lock()
            .expect("script")
            .pop_front()
            .unwrap_or_else(|| {
                vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]
            });
        Ok(next)
    }
}

struct AllowPrompter;

impl PermissionPrompter for AllowPrompter {
    fn decide<'a>(
        &'a self,
        _request: AsyncPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>>
    {
        Box::pin(async { Ok(AsyncPermissionDecision::Allow) })
    }
}

fn bash_call(id: &str, command: &str) -> Vec<AssistantEvent> {
    vec![
        AssistantEvent::ToolUse {
            id: id.to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": command }).to_string(),
        },
        AssistantEvent::MessageStop,
    ]
}

fn edit_call(id: &str, path: &str) -> Vec<AssistantEvent> {
    vec![
        AssistantEvent::ToolUse {
            id: id.to_string(),
            name: "edit_file".to_string(),
            input: serde_json::json!({ "path": path }).to_string(),
        },
        AssistantEvent::MessageStop,
    ]
}

fn text_stop(text: &str) -> Vec<AssistantEvent> {
    vec![
        AssistantEvent::TextDelta(text.to_string()),
        AssistantEvent::MessageStop,
    ]
}

/// Every text block of a request, flattened — the reminders ride `messages` as a
/// trailing `System` message, so this is where an injected observation lands.
fn request_text(request: &ApiRequest) -> String {
    request
        .messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct Harness {
    runtime: ConversationRuntime<ScriptedClient, StaticToolExecutor>,
    requests: Arc<Mutex<Vec<ApiRequest>>>,
}

/// A runtime whose bash dispatch always reports a clean foreground exit — the
/// shape `bash_result_exited_zero` accepts (`stdout` present, no
/// `returnCodeInterpretation`, not interrupted, not backgrounded).
fn harness(script: Vec<Vec<AssistantEvent>>) -> Harness {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = ScriptedClient {
        script: Arc::new(Mutex::new(script.into_iter().collect())),
        requests: Arc::clone(&requests),
    };
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        client,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    let dispatch: ConcurrentDispatchFn = Arc::new(|name, _input| {
        if name == "edit_file" {
            Ok(r#"{"filePath":"/repo/src/lib.rs"}"#.to_string())
        } else {
            Ok(r#"{"stdout":"test result: ok. 3 passed","stderr":""}"#.to_string())
        }
    });
    runtime.set_concurrent_dispatch(dispatch);
    Harness { runtime, requests }
}

async fn run_turn(harness: &mut Harness, input: &str) {
    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain = tokio::spawn(async move {
        let mut blocks: Vec<RenderBlock> = Vec::new();
        while let Some(block) = rx.recv().await {
            blocks.push(block);
        }
        blocks
    });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(AllowPrompter);
    harness
        .runtime
        .run_turn_streaming(input, tx, prompter)
        .await
        .expect("turn completes");
    drain.await.expect("drain");
}

/// A green check observed in turn 1 is on the wire at the start of turn 2, in
/// the FRESH form, and the firing is attested exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_green_check_reaches_the_next_turns_wire_and_attests_once() {
    let _serial = ATTEST_SERIAL.lock().await;
    let firings_before = observation_firings();
    let mut harness = harness(vec![
        bash_call("t1", "cargo test -p runtime"),
        text_stop("green"),
        text_stop("next stage"),
    ]);

    run_turn(&mut harness, "verify the crate").await;
    let after_first = harness.requests.lock().expect("requests").len();
    // Nothing to observe yet on turn 1 — the ledger was empty when it started.
    for (index, request) in harness
        .requests
        .lock()
        .expect("requests")
        .iter()
        .enumerate()
    {
        assert!(
            !request_text(request).contains(VERIFIED_STATE_REMINDER_PREFIX),
            "turn 1 request {index} must carry no observation"
        );
    }

    run_turn(&mut harness, "now do stage two").await;
    let requests = harness.requests.lock().expect("requests");
    let turn_two = requests
        .get(after_first)
        .expect("turn two issued a request");
    let text = request_text(turn_two);
    assert!(
        text.contains(VERIFIED_STATE_REMINDER_PREFIX),
        "the observation must reach turn two's wire: {text}"
    );
    assert!(
        text.contains("`cargo test -p runtime` — ran green, nothing edited since"),
        "the fresh form must name the command it observed: {text}"
    );
    assert!(
        text.contains("does NOT know which files a command covers"),
        "the harness must decline the coverage claim it cannot make: {text}"
    );

    // It rides `messages` (an append behind the new user turn), never the
    // frozen system prompt — the prefix-cache contract every reminder obeys.
    assert!(
        turn_two
            .system_prompt
            .iter()
            .all(|block| !block.contains(VERIFIED_STATE_REMINDER_PREFIX)),
        "the observation must not rewrite the cached system prefix"
    );
    assert!(
        turn_two
            .wire_reminders
            .iter()
            .all(|reminder| !reminder.contains(VERIFIED_STATE_REMINDER_PREFIX)),
        "the observation is absorbed into the transcript, not sent wire-only"
    );

    assert_eq!(
        observation_firings() - firings_before,
        1,
        "exactly one turn start carried an observation"
    );
}

/// A turn that edits after the check flips the next turn's line to the
/// invalidated form and names the file — and while that mutating turn is still
/// running, the now-false "nothing edited since" block is not re-asserted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_invalidates_the_line_and_retires_the_standing_block() {
    let _serial = ATTEST_SERIAL.lock().await;
    // Turn two edits FIRST and then keeps working for enough tool batches to
    // drift past the absorber's dedupe window — which is precisely when an
    // unchanged reminder set is re-appended near the tail. Without that drift
    // the "exactly once" assertion below cannot fail, and a check that cannot
    // fail is not a check. (Verified by neutralizing the retirement: this test
    // goes red, the short-turn version did not.)
    let mut script = vec![
        bash_call("t1", "cargo test -p runtime"),
        text_stop("green"),
        edit_call("e1", "/repo/src/lib.rs"),
    ];
    for index in 0..MID_TURN_BATCHES {
        // Distinct commands on purpose: identical calls would trip the tool
        // repetition guard and end the turn before it drifts far enough.
        script.push(bash_call(&format!("n{index}"), &format!("ls -la dir{index}")));
    }
    script.push(text_stop("patched"));
    script.push(text_stop("stage three"));
    let mut harness = harness(script);

    run_turn(&mut harness, "verify the crate").await;
    let before_second = harness.requests.lock().expect("requests").len();
    run_turn(&mut harness, "now patch it").await;
    let after_second = harness.requests.lock().expect("requests").len();
    run_turn(&mut harness, "and now stage three").await;

    let requests = harness.requests.lock().expect("requests");
    // Turn two opened with the FRESH block (nothing had been edited yet), and
    // after its edit no further request re-asserted it: exactly one copy rides
    // the transcript, anchored where it was true.
    for (offset, request) in requests[before_second..after_second].iter().enumerate() {
        assert_eq!(
            request_text(request)
                .matches(VERIFIED_STATE_REMINDER_PREFIX)
                .count(),
            1,
            "turn two request {offset} must carry the turn-start block exactly once"
        );
    }

    let turn_three = requests.get(after_second).expect("turn three request");
    let text = request_text(turn_three);
    assert!(
        text.contains(
            "`cargo test -p runtime` — ran green BUT these files changed since: /repo/src/lib.rs"
        ),
        "the edit must invalidate the line and name the file: {text}"
    );
}

/// A session that never ran a check sends bytes identical to a build without
/// this feature — and says so in the ledger rather than going silent, so a
/// zero firing count can be told apart from a broken injection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_without_a_green_check_stays_byte_neutral() {
    let _serial = ATTEST_SERIAL.lock().await;
    let firings_before = observation_firings();
    let declines_before = observation_declines();
    let mut harness = harness(vec![
        // Green, but `ls` is not a check: running it proves nothing about the
        // tree, so it must not license an observation.
        bash_call("t1", "ls -la"),
        text_stop("looked around"),
        text_stop("carried on"),
    ]);

    run_turn(&mut harness, "look around").await;
    run_turn(&mut harness, "and again").await;

    for (index, request) in harness
        .requests
        .lock()
        .expect("requests")
        .iter()
        .enumerate()
    {
        assert!(
            !request_text(request).contains(VERIFIED_STATE_REMINDER_PREFIX),
            "request {index} must be byte-neutral without a green check"
        );
    }
    assert_eq!(
        observation_firings() - firings_before,
        0,
        "no green check on record must mean no firing"
    );
    assert!(
        observation_declines() > declines_before,
        "an empty ledger must record WHY it stayed silent, not just stay silent"
    );
}
