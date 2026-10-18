use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;
use zerocode_core::surface_execution::{SurfaceEffect, SurfaceExecution, MAX_SURFACE_ID_BYTES};

use super::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, Attendance, ConcurrentResultDispatchFn,
    ConversationRuntime, ResolvedToolCall, RuntimeError, ToolError, ToolExecutionResult,
    ToolExecutor,
};
use crate::message_stream::types::{BlockId, RenderBlock};
use crate::permission::{
    PermissionDecision, PermissionError, PermissionPrompter, PermissionRequest,
};
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::session::Session;

struct PlannedCall {
    name: String,
    input: String,
    result: Result<ToolExecutionResult, ToolError>,
}

#[derive(Clone)]
struct ScriptApi {
    batches: Arc<Mutex<VecDeque<Vec<AssistantEvent>>>>,
}

impl ScriptApi {
    fn next(&self) -> Vec<AssistantEvent> {
        self.batches.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![
                AssistantEvent::TextDelta("finished".to_string()),
                AssistantEvent::MessageStop,
            ]
        })
    }
}

impl ApiClient for ScriptApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(self.next())
    }
}

impl AsyncApiClient for ScriptApi {
    fn stream_async<'a>(
        &'a self,
        _request: ApiRequest,
        _render_tx: mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async { Ok(self.next()) })
    }
}

type ScriptReplies = BTreeMap<(String, String), VecDeque<Result<ToolExecutionResult, ToolError>>>;

struct ScriptState {
    replies: ScriptReplies,
    executed: Vec<ResolvedToolCall>,
}

#[derive(Clone)]
struct ScriptExecutor {
    state: Arc<Mutex<ScriptState>>,
}

impl ScriptExecutor {
    fn dispatch(&self, name: &str, input: &str) -> Result<ToolExecutionResult, ToolError> {
        let resolved = self
            .resolve_call(name, input)?
            .unwrap_or_else(|| ResolvedToolCall {
                name: name.to_string(),
                input: input.to_string(),
            });
        let mut state = self.state.lock().unwrap();
        state.executed.push(resolved);
        state
            .replies
            .get_mut(&(name.to_string(), input.to_string()))
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| Err(ToolError::new("no scripted execution result")))
    }

    fn observations(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .executed
            .iter()
            .filter(|call| {
                call.name == "Computer"
                    && serde_json::from_str::<Value>(&call.input).is_ok_and(|input| {
                        input.get("action").and_then(Value::as_str) == Some("observe")
                    })
            })
            .count()
    }
}

impl ToolExecutor for ScriptExecutor {
    fn execute(&mut self, name: &str, input: &str) -> Result<String, ToolError> {
        self.dispatch(name, input).map(|result| result.text)
    }

    fn execute_result(
        &mut self,
        name: &str,
        input: &str,
    ) -> Result<ToolExecutionResult, ToolError> {
        self.dispatch(name, input)
    }

    fn resolve_call(&self, name: &str, input: &str) -> Result<Option<ResolvedToolCall>, ToolError> {
        if name != "CapabilityInvoke" {
            return Ok(None);
        }
        let value: Value =
            serde_json::from_str(input).map_err(|error| ToolError::new(error.to_string()))?;
        let inner_name = value
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| ToolError::new("missing inner tool name"))?;
        if inner_name == "CapabilityInvoke" {
            return Err(ToolError::new("CapabilityInvoke cannot invoke itself"));
        }
        let inner_input = value.get("input").cloned().unwrap_or_else(|| json!({}));
        if !inner_input.is_object() {
            return Err(ToolError::new("inner input must be an object"));
        }
        Ok(Some(ResolvedToolCall {
            name: inner_name.to_string(),
            input: inner_input.to_string(),
        }))
    }
}

struct AllowPrompt;

impl PermissionPrompter for AllowPrompt {
    fn decide<'a>(
        &'a self,
        _request: PermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PermissionDecision, PermissionError>> + Send + 'a>>
    {
        Box::pin(async { Ok(PermissionDecision::Allow) })
    }
}

fn computer(action: &str, window: u64, revision: u64, effect: SurfaceEffect) -> PlannedCall {
    let input = if action == "observe" {
        json!({"action": action, "app": "Fixture", "window_id": window})
    } else {
        json!({"action": action, "app": "Fixture", "window_id": window, "label": "Next"})
    };
    PlannedCall {
        name: "Computer".to_string(),
        input: input.to_string(),
        result: Ok(ToolExecutionResult {
            text: "host result".to_string(),
            surface: Some(SurfaceExecution {
                surface_id: format!("fixture-host/computer/{window}/lease-a"),
                state_revision: revision,
                effect,
            }),
        }),
    }
}

fn observe(revision: u64) -> PlannedCall {
    computer("observe", 41, revision, SurfaceEffect::Observation)
}

fn confirm(previous_revision: u64, revision: u64) -> PlannedCall {
    computer(
        "left_click",
        41,
        revision,
        SurfaceEffect::ConfirmedChange { previous_revision },
    )
}

fn wrapped(mut call: PlannedCall, nonce: Option<u64>) -> PlannedCall {
    let mut input = json!({
        "name": call.name,
        "input": serde_json::from_str::<Value>(&call.input).unwrap(),
    });
    if let Some(nonce) = nonce {
        input["nonce"] = nonce.into();
    }
    call.name = "CapabilityInvoke".to_string();
    call.input = input.to_string();
    call
}

fn runtime_for(
    calls: Vec<PlannedCall>,
) -> (
    ConversationRuntime<ScriptApi, ScriptExecutor>,
    ScriptExecutor,
) {
    runtime_for_batches(calls.into_iter().map(|call| vec![call]).collect())
}

fn runtime_for_batches(
    planned_batches: Vec<Vec<PlannedCall>>,
) -> (
    ConversationRuntime<ScriptApi, ScriptExecutor>,
    ScriptExecutor,
) {
    let mut batches = VecDeque::new();
    let mut replies: ScriptReplies = BTreeMap::new();
    for (batch_index, calls) in planned_batches.into_iter().enumerate() {
        let mut events = Vec::new();
        for (call_index, call) in calls.into_iter().enumerate() {
            events.push(AssistantEvent::ToolUse {
                id: format!("surface-call-{batch_index}-{call_index}"),
                name: call.name.clone(),
                input: call.input.clone(),
            });
            replies
                .entry((call.name, call.input))
                .or_default()
                .push_back(call.result);
        }
        events.push(AssistantEvent::MessageStop);
        batches.push_back(events);
    }
    let api = ScriptApi {
        batches: Arc::new(Mutex::new(batches)),
    };
    let executor = ScriptExecutor {
        state: Arc::new(Mutex::new(ScriptState {
            replies,
            executed: Vec::new(),
        })),
    };
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        api.clone(),
        executor.clone(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    )
    .with_max_iterations(48);
    runtime.set_max_tool_calls(64);
    runtime.set_attendance(Attendance::Unattended);
    runtime.set_async_api_client(Arc::new(api));
    (runtime, executor)
}

#[test]
fn same_surface_confirmed_changes_allow_identical_observations() {
    let mut calls = Vec::new();
    for revision in 1..=6 {
        calls.push(observe(revision));
        if revision < 6 {
            calls.push(confirm(revision, revision + 1));
        }
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Observe and confirm the selected window", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        6,
        "same-surface confirmation must reach the real executor"
    );
}

#[test]
fn confirmed_same_batch_progress_removes_only_its_own_sticky_stop() {
    let (mut runtime, executor) = runtime_for_batches(vec![
        vec![observe(1), observe(1), observe(1)],
        vec![observe(1), confirm(1, 2), observe(2)],
        vec![observe(2)],
    ]);
    runtime
        .run_turn(
            "Continue after a same-batch confirmation of the stopped surface",
            None,
        )
        .unwrap();
    assert_eq!(
        executor.observations(),
        5,
        "the confirmed surface must reach both the rest of its batch and the next batch"
    );
}

#[test]
fn confirmed_other_surface_progress_keeps_the_original_sticky_stop() {
    let observed_other = || computer("observe", 52, 1, SurfaceEffect::Observation);
    let (mut runtime, executor) = runtime_for_batches(vec![
        vec![
            observe(1),
            observed_other(),
            observe(1),
            observed_other(),
            observe(1),
            observed_other(),
        ],
        vec![
            observe(1),
            observed_other(),
            computer(
                "left_click",
                52,
                2,
                SurfaceEffect::ConfirmedChange {
                    previous_revision: 1,
                },
            ),
            computer("observe", 52, 2, SurfaceEffect::Observation),
        ],
        vec![computer("observe", 52, 2, SurfaceEffect::Observation)],
    ]);
    runtime
        .run_turn(
            "Keep another surface's earlier stop after unrelated same-batch progress",
            None,
        )
        .unwrap();
    assert_eq!(
        executor.observations(),
        7,
        "a different surface must not remove the original terminating stop"
    );
}

#[test]
fn capability_invocation_keeps_same_surface_confirmation() {
    let mut calls = Vec::new();
    for revision in 1..=6 {
        calls.push(wrapped(observe(revision), None));
        if revision < 6 {
            calls.push(wrapped(confirm(revision, revision + 1), None));
        }
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Observe the selected window through its capability", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        6,
        "a wrapper must preserve actual surface progress"
    );
}

#[test]
fn changed_observations_allow_fresh_reads_without_recording_confirmed_effects() {
    let (mut runtime, executor) = runtime_for((1..=6).map(observe).collect());
    runtime
        .run_turn("Observe fresh states of the selected window", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        6,
        "fresh host state must distinguish identical read inputs"
    );
    assert_eq!(
        runtime.turn_progress_results, 0,
        "observation freshness is not a confirmed effect"
    );
}

#[test]
fn unchanged_surface_observations_remain_bounded() {
    let (mut runtime, executor) = runtime_for((0..9).map(|_| observe(1)).collect());
    runtime
        .run_turn("Observe the same selected window", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "unchanged observations must stop before another dispatch"
    );
}

#[test]
fn another_surface_cannot_forgive_repeated_observations() {
    let mut calls = Vec::new();
    for revision in 1..=6 {
        calls.push(observe(1));
        calls.push(computer(
            "left_click",
            52,
            revision + 1,
            SurfaceEffect::ConfirmedChange {
                previous_revision: revision,
            },
        ));
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Keep the originally selected window", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "another window must not release the original guard"
    );
}

#[test]
fn a_file_edit_cannot_forgive_a_surface_observation_loop() {
    let mut calls = Vec::new();
    for _ in 0..6 {
        calls.push(observe(1));
        calls.push(PlannedCall {
            name: "edit_file".to_string(),
            input: json!({"path": "fixture.rs"}).to_string(),
            result: Ok("edited fixture".to_string().into()),
        });
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Keep the unchanged surface bounded while editing", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "file forgiveness must not include surface observations"
    );
}

#[test]
fn a_pathless_file_mutation_cannot_forgive_surface_observations_without_metadata() {
    let mut calls = Vec::new();
    for _ in 0..6 {
        let mut observation = observe(1);
        observation.result = Ok("old host observation without metadata".to_string().into());
        calls.push(observation);
        calls.push(PlannedCall {
            name: "NotebookEdit".to_string(),
            input: json!({"notebook_path":"fixture.ipynb"}).to_string(),
            result: Ok("edited notebook fixture".to_string().into()),
        });
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn(
            "Keep surface repeats bounded across pathless file mutations",
            None,
        )
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "a mutation without path must not clear Computer repetition"
    );
}

#[test]
fn capability_nonce_changes_do_not_buy_unchanged_observations() {
    let calls = (1..=9)
        .map(|nonce| wrapped(observe(1), Some(nonce)))
        .collect();
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn(
            "Observe the same capability without changing its state",
            None,
        )
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "wrapper-only input must not create a fresh observation"
    );
}

#[test]
fn changing_ignored_input_fields_cannot_create_unbounded_same_surface_observations() {
    let calls = (0..9).map(|nonce| {
        let mut call = observe(1);
        call.input = json!({"action":"observe","app":"Fixture","window_id":41,"text":format!("ignored-{nonce}")}).to_string();
        call
    }).collect();
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn(
            "Keep unchanged actual surface observations bounded despite ignored input",
            None,
        )
        .unwrap();
    assert!(
        executor.observations() <= 4,
        "new raw inputs must still share the actual surface's observation count"
    );
}

#[test]
fn an_initial_host_baseline_does_not_reset_an_already_counted_legacy_observation() {
    let mut initial = observe(0);
    initial.result = Ok("legacy result before host metadata".to_string().into());
    let (mut runtime, executor) = runtime_for(vec![
        initial,
        observe(0),
        observe(0),
        observe(0),
        observe(0),
    ]);
    runtime
        .run_turn(
            "Keep the original count when host metadata first becomes available",
            None,
        )
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "a first zero baseline must migrate rather than clear the old count"
    );
}

#[test]
fn direct_and_wrapped_calls_share_the_same_unchanged_observation_limit() {
    let calls = (0..9)
        .map(|index| {
            if index % 2 == 0 {
                observe(1)
            } else {
                wrapped(observe(1), None)
            }
        })
        .collect();
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Keep direct and wrapped observation retries bounded", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "changing the addressing wrapper must not create another limit"
    );
}

#[test]
fn metadata_shaped_body_text_is_not_surface_evidence() {
    let calls = (1..=9)
        .map(|revision| {
            let mut call = observe(1);
            call.result = Ok(ToolExecutionResult {
                text: json!({"surfaceExecution": {
                    "surfaceId": "fixture-host/computer/41/lease-a",
                    "stateRevision": revision,
                    "effect": {"kind": "confirmedChange", "previousRevision": revision - 1},
                }})
                .to_string(),
                surface: None,
            });
            call
        })
        .collect();
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Read untrusted result text", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "a result body cannot claim a host effect"
    );
}

#[test]
fn failed_actions_do_not_release_armed_observations() {
    let mut failed = confirm(1, 2);
    failed.result = Err(ToolError::new("fixture action failed"));
    let (mut runtime, executor) =
        runtime_for(vec![observe(1), observe(1), observe(1), failed, observe(1)]);
    runtime
        .run_turn("Confirm a failed action without assuming progress", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "an action failure must preserve the armed repeat"
    );
}

#[test]
fn attended_failed_observations_are_skipped_without_ending_the_turn() {
    let calls = (0..6)
        .map(|_| {
            let mut call = observe(1);
            call.result = Err(ToolError::new("fixture observation failed"));
            call
        })
        .collect();
    let (mut runtime, executor) = runtime_for(calls);
    runtime.set_attendance(Attendance::Attended);
    let summary = runtime
        .run_turn(
            "Keep failed observations bounded while a person attends",
            None,
        )
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "an error result is not evicted observation context"
    );
    assert_eq!(
        summary.iterations, 7,
        "the attended turn must still reach its scripted natural end"
    );
}

#[test]
fn uncertain_actions_do_not_claim_a_changed_state() {
    let uncertain = computer("left_click", 41, 2, SurfaceEffect::Uncertain);
    let (mut runtime, executor) = runtime_for(vec![
        observe(1),
        observe(1),
        observe(1),
        uncertain,
        observe(2),
    ]);
    runtime
        .run_turn(
            "Keep an uncertain action distinct from a confirmed effect",
            None,
        )
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "an uncertain action cannot clear an armed observation"
    );
}

#[test]
fn acknowledgements_allow_one_confirmation_of_the_same_revision() {
    let acknowledged = computer("left_click", 41, 1, SurfaceEffect::Acknowledged);
    let (mut runtime, executor) = runtime_for(vec![
        observe(1),
        observe(1),
        observe(1),
        acknowledged,
        observe(1),
        observe(1),
    ]);
    runtime
        .run_turn("Confirm the acknowledged action once", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        4,
        "one bounded confirmation must reach the original surface"
    );
}

#[test]
fn repeated_acknowledgements_cannot_reissue_confirmation_credit() {
    let mut calls = vec![observe(1), observe(1), observe(1)];
    for _ in 0..6 {
        calls.push(computer("left_click", 41, 1, SurfaceEffect::Acknowledged));
        calls.push(observe(1));
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Keep same-state acknowledgement retries bounded", None)
        .unwrap();
    assert!(
        executor.observations() <= 4,
        "acknowledgement credit must not be replenished without a new state"
    );
}

#[test]
fn mismatched_previous_revision_cannot_claim_confirmed_progress() {
    let (mut runtime, executor) = runtime_for(vec![
        observe(7),
        observe(7),
        observe(7),
        confirm(1, 8),
        observe(8),
    ]);
    runtime
        .run_turn("Reject an effect for a different original state", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "the confirmed effect must start from the observed state"
    );
}

#[test]
fn malformed_surface_evidence_cannot_forgive_armed_observations() {
    let identity = "fixture-host/computer/41/lease-a";
    for invalid in [
        SurfaceExecution {
            surface_id: String::new(),
            state_revision: 2,
            effect: SurfaceEffect::ConfirmedChange {
                previous_revision: 1,
            },
        },
        SurfaceExecution {
            surface_id: "s".repeat(MAX_SURFACE_ID_BYTES + 1),
            state_revision: 2,
            effect: SurfaceEffect::ConfirmedChange {
                previous_revision: 1,
            },
        },
        SurfaceExecution {
            surface_id: identity.to_string(),
            state_revision: 1,
            effect: SurfaceEffect::ConfirmedChange {
                previous_revision: 1,
            },
        },
        SurfaceExecution {
            surface_id: identity.to_string(),
            state_revision: 1,
            effect: SurfaceEffect::ConfirmedChange {
                previous_revision: 2,
            },
        },
    ] {
        let invalid_reply = PlannedCall {
            name: "Computer".to_string(),
            input: json!({"action":"left_click","app":"Fixture","window_id":41,"label":"Next"})
                .to_string(),
            result: Ok(ToolExecutionResult {
                text: "invalid host metadata".to_string(),
                surface: Some(invalid),
            }),
        };
        let (mut runtime, executor) = runtime_for(vec![
            observe(1),
            observe(1),
            observe(1),
            invalid_reply,
            observe(2),
        ]);
        runtime
            .run_turn("Reject malformed or non-forward host evidence", None)
            .unwrap();
        assert_eq!(
            executor.observations(),
            3,
            "invalid metadata must not release an armed observation"
        );
        assert_eq!(
            runtime.turn_progress_results, 0,
            "invalid metadata must not create objective progress"
        );
    }
}

#[test]
fn zero_revision_baselines_do_not_create_progress_or_remove_limits() {
    let (mut runtime, executor) = runtime_for((0..9).map(|_| observe(0)).collect());
    runtime
        .run_turn("Keep initial host baselines bounded", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "a zero baseline is not a new state on every call"
    );
    assert_eq!(
        runtime.turn_progress_results, 0,
        "an initial baseline must not count as an effect"
    );
}

#[test]
fn a_confirmed_change_can_start_from_a_zero_revision_baseline() {
    let mut calls = Vec::new();
    for revision in 0..6 {
        calls.push(observe(revision));
        if revision < 5 {
            calls.push(confirm(revision, revision + 1));
        }
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Confirm real changes after an initial host baseline", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        6,
        "a real confirmation from revision zero must retain the same surface"
    );
}

#[test]
fn a_recreated_zero_baseline_cannot_erase_a_newer_surface_state() {
    let old_baseline = computer("read", 41, 0, SurfaceEffect::Observation);
    let (mut runtime, executor) = runtime_for(vec![
        observe(7),
        observe(7),
        observe(7),
        old_baseline,
        observe(7),
    ]);
    runtime
        .run_turn(
            "Ignore a recreated baseline after a newer observed revision",
            None,
        )
        .unwrap();
    assert_eq!(
        executor.observations(),
        3,
        "a baseline cache miss must not release the previous state"
    );
}

#[test]
fn replacing_the_session_invalidates_the_previous_observation_evidence() {
    let (mut runtime, executor) = runtime_for(vec![observe(7)]);
    runtime
        .run_turn("Observe in the original session", None)
        .unwrap();
    runtime.replace_session(Session::new());
    let next = confirm(7, 8);
    runtime.api_client.batches.lock().unwrap().push_back(vec![
        AssistantEvent::ToolUse {
            id: "new-session-confirmation".to_string(),
            name: next.name.clone(),
            input: next.input.clone(),
        },
        AssistantEvent::MessageStop,
    ]);
    executor
        .state
        .lock()
        .unwrap()
        .replies
        .entry((next.name, next.input))
        .or_default()
        .push_back(next.result);
    runtime
        .run_turn(
            "Confirm without borrowing the old session's observation",
            None,
        )
        .unwrap();
    assert_eq!(
        executor.state.lock().unwrap().executed.len(),
        2,
        "the new confirmation must actually execute"
    );
    assert_eq!(
        runtime.turn_progress_results, 0,
        "a replaced session must not inherit prior observation evidence"
    );
}

#[test]
fn duplicate_confirmed_effects_do_not_keep_forgiving_observations() {
    let mut calls = vec![observe(1), observe(1), observe(1)];
    for _ in 0..8 {
        calls.push(confirm(1, 2));
        calls.push(observe(2));
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Consume each confirmed effect once", None)
        .unwrap();
    assert!(
        executor.observations() <= 6,
        "replaying a confirmed revision must not keep resetting its observation limit"
    );
}

#[test]
fn nested_capabilities_never_reach_the_surface_executor() {
    let mut nested = wrapped(observe(1), None);
    nested.input = json!({"name": "CapabilityInvoke", "input": {
        "name": "Computer", "input": {"action": "observe", "app": "Fixture", "window_id": 41}
    }})
    .to_string();
    let (mut runtime, executor) = runtime_for(vec![nested]);
    runtime
        .run_turn("Reject nested capability invocation", None)
        .unwrap();
    assert_eq!(
        executor.observations(),
        0,
        "nested wrappers must not dispatch a surface action"
    );
}

#[tokio::test]
async fn streaming_surface_confirmations_keep_structured_results() {
    let mut calls = Vec::new();
    for revision in 1..=6 {
        calls.push(observe(revision));
        if revision < 6 {
            calls.push(confirm(revision, revision + 1));
        }
    }
    let (mut runtime, executor) = runtime_for(calls);
    let (render_tx, mut render_rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
    runtime
        .run_turn_streaming(
            "Observe and confirm the selected window",
            render_tx,
            Arc::new(AllowPrompt),
        )
        .await
        .unwrap();
    drain.await.unwrap();
    assert_eq!(
        executor.observations(),
        6,
        "streaming must preserve the same per-call evidence"
    );
}

#[tokio::test]
async fn concurrent_surface_confirmations_use_the_result_dispatcher() {
    let mut calls = Vec::new();
    for revision in 1..=6 {
        calls.push(observe(revision));
        if revision < 6 {
            calls.push(confirm(revision, revision + 1));
        }
    }
    let (mut runtime, executor) = runtime_for(calls);
    let legacy = executor.clone();
    runtime.set_concurrent_dispatch(Arc::new(move |name, input| {
        legacy.dispatch(name, input).map(|result| result.text)
    }));
    let structured = executor.clone();
    runtime.set_concurrent_result_dispatch(Arc::new(move |name, input| {
        structured.dispatch(name, input)
    }));
    let (render_tx, mut render_rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
    runtime
        .run_turn_streaming(
            "Confirm through the live dispatcher",
            render_tx,
            Arc::new(AllowPrompt),
        )
        .await
        .unwrap();
    drain.await.unwrap();
    assert_eq!(
        executor.observations(),
        6,
        "the live dispatcher must retain actual surface evidence"
    );
}

#[tokio::test]
async fn reversed_wave_completions_keep_each_surfaces_evidence() {
    let mut batches = Vec::new();
    for revision in 1..=6 {
        let effect = if revision == 1 {
            SurfaceEffect::Observation
        } else {
            SurfaceEffect::ConfirmedChange {
                previous_revision: revision - 1,
            }
        };
        batches.push(vec![
            PlannedCall {
                name: "read_file".to_string(),
                input: json!({"path":"fixture-a.rs"}).to_string(),
                result: Ok(ToolExecutionResult {
                    text: "surface a".to_string(),
                    surface: Some(SurfaceExecution {
                        surface_id: "fixture-host/surface-a/lease-a".to_string(),
                        state_revision: revision,
                        effect,
                    }),
                }),
            },
            PlannedCall {
                name: "read_file".to_string(),
                input: json!({"path":"fixture-b.rs"}).to_string(),
                result: Ok(ToolExecutionResult {
                    text: "surface b".to_string(),
                    surface: Some(SurfaceExecution {
                        surface_id: "fixture-host/surface-b/lease-a".to_string(),
                        state_revision: 1,
                        effect: SurfaceEffect::Observation,
                    }),
                }),
            },
        ]);
    }
    let (mut runtime, executor) = runtime_for_batches(batches);
    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    let completed_rx = Mutex::new(completed_rx);
    let awaited = AtomicUsize::new(0);
    let dispatched = executor.clone();
    let dispatch: ConcurrentResultDispatchFn = Arc::new(move |name, input| {
        let is_first = input.contains("fixture-a.rs");
        let result = dispatched.dispatch(name, input);
        if is_first && awaited.fetch_add(1, Ordering::SeqCst) < 3 {
            completed_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .map_err(|error| ToolError::new(error.to_string()))?;
        } else if !is_first {
            let _ = completed_tx.send(());
        }
        result
    });
    runtime.set_concurrent_result_dispatch(dispatch);
    let (render_tx, mut render_rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
    runtime
        .run_turn_streaming(
            "Keep another surface bounded while completion order reverses",
            render_tx,
            Arc::new(AllowPrompt),
        )
        .await
        .unwrap();
    drain.await.unwrap();
    let unchanged = executor
        .state
        .lock()
        .unwrap()
        .executed
        .iter()
        .filter(|call| call.input.contains("fixture-b.rs"))
        .count();
    assert_eq!(
        unchanged, 3,
        "another completed future's effect must not attach to the unchanged surface"
    );
}

#[tokio::test]
async fn cancelled_late_confirmation_cannot_become_the_next_calls_effect() {
    let (mut runtime, executor) =
        runtime_for(vec![observe(1), observe(1), observe(1), confirm(1, 2)]);
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started_tx = Mutex::new(Some(started_tx));
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let finished_tx = Mutex::new(Some(finished_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let dispatched = executor.clone();
    let dispatch: ConcurrentResultDispatchFn = Arc::new(move |name, input| {
        let is_action = serde_json::from_str::<Value>(input)
            .is_ok_and(|input| input.get("action").and_then(Value::as_str) == Some("left_click"));
        if is_action {
            if let Some(started) = started_tx.lock().unwrap().take() {
                let _ = started.send(());
            }
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .map_err(|error| ToolError::new(error.to_string()))?;
        }
        let result = dispatched.dispatch(name, input);
        if is_action {
            if let Some(finished) = finished_tx.lock().unwrap().take() {
                let _ = finished.send(());
            }
        }
        result
    });
    let legacy = Arc::clone(&dispatch);
    runtime.set_concurrent_dispatch(Arc::new(move |name, input| {
        legacy(name, input).map(|result| result.text)
    }));
    runtime.set_concurrent_result_dispatch(dispatch);
    let cancel = runtime.tool_cancel_signal();
    let controller = async move {
        started_rx.await.unwrap();
        cancel.cancel_running_tools();
    };
    let (render_tx, mut render_rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
    let (turn, ()) = tokio::join!(
        runtime.run_turn_streaming(
            "Cancel an in-flight surface action",
            render_tx,
            Arc::new(AllowPrompt)
        ),
        controller
    );
    let _ = release_tx.send(());
    finished_rx.await.unwrap();
    turn.unwrap();
    drain.await.unwrap();

    let next = observe(1);
    runtime.api_client.batches.lock().unwrap().push_back(vec![
        AssistantEvent::ToolUse {
            id: "after-cancel".to_string(),
            name: next.name.clone(),
            input: next.input.clone(),
        },
        AssistantEvent::MessageStop,
    ]);
    executor
        .state
        .lock()
        .unwrap()
        .replies
        .entry((next.name, next.input))
        .or_default()
        .push_back(Ok("next call has no surface proof".to_string().into()));
    let (render_tx, mut render_rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
    runtime
        .run_turn_streaming(
            "Observe after cancellation without reusing old evidence",
            render_tx,
            Arc::new(AllowPrompt),
        )
        .await
        .unwrap();
    drain.await.unwrap();
    assert_eq!(
        executor.observations(),
        4,
        "the next call must execute before checking for leaked evidence"
    );
    assert_eq!(
        runtime.turn_progress_results, 0,
        "a cancelled future's late confirmation must not attach to another call"
    );
}
