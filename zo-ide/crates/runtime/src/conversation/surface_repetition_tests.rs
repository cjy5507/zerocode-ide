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
    resolutions: Option<Arc<AtomicUsize>>,
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
        if let Some(resolutions) = &self.resolutions { resolutions.fetch_add(1, Ordering::Relaxed); }
        if name == "Bash" {
            let value: Value = serde_json::from_str(input).map_err(|error| ToolError::new(error.to_string()))?;
            if let Some(words) = value.get("command").and_then(Value::as_str).and_then(crate::bash_validation::literal_argv) {
                if words.first().map(String::as_str) == Some("zerocode-browser") {
                    return Ok(Some(ResolvedToolCall { name:"zerocode-browser".into(), input:serde_json::to_string(&words[1..]).unwrap() }));
                }
            }
        }
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
        resolutions: Some(Arc::new(AtomicUsize::new(0))),
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
fn browser_control_revision_spelling_cannot_buy_unchanged_observations() {
    let calls = (1..12)
        .map(|width| PlannedCall {
            name: zerocode_core::agent_browser::BROWSER_CLI.into(),
            input: serde_json::to_string(&vec![
                "read".to_string(),
                "browser-1".to_string(),
                "--workspace-revision".to_string(),
                format!("{value:0width$}", value = 7),
            ])
            .unwrap(),
            result: Ok(ToolExecutionResult {
                text: "same owned browser page".into(),
                surface: Some(SurfaceExecution {
                    surface_id: "fixture-host/browser-1/interval-7".into(),
                    state_revision: 0,
                    effect: SurfaceEffect::Observation,
                }),
            }),
        })
        .collect();
    let (mut runtime, executor) = runtime_for(calls);
    runtime
        .run_turn("Keep observing the same selected browser", None)
        .unwrap();
    assert_eq!(
        executor.state.lock().unwrap().executed.len(),
        3,
        "zero padding an unchanged control revision must not bypass the observation guard"
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
    runtime.set_attendance(Attendance::Attended);
    runtime
        .run_turn(
            "Keep unchanged actual surface observations bounded despite ignored input",
            None,
        )
        .unwrap();
    assert_eq!(executor.observations(), 3, "a new spelling of an observed target must be skipped before execution");
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
    runtime.set_attendance(Attendance::Attended);
    runtime
        .run_turn("Keep same-state acknowledgement retries bounded", None)
        .unwrap();
    assert_eq!(executor.observations(), 4, "same-state acknowledgement grants exactly one confirmation in an attended turn");
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


#[test]
fn unrecorded_calls_cannot_fill_the_surface_table_and_refuse_a_new_observation() {
    assert_new_observation_after_six_hundred_calls(false);
}

#[test]
fn attested_calls_past_the_binding_limit_do_not_refuse_a_new_observation() {
    assert_new_observation_after_six_hundred_calls(true);
}

fn assert_new_observation_after_six_hundred_calls(records: bool) {
    use super::repetition::ToolBatchRepetitionHardStops;
    let (mut runtime, _) = runtime_for(Vec::new());
    let mut batch = ToolBatchRepetitionHardStops::default();
    for index in 0..600 {
        let input = json!({"action":"observe", "app":"Fixture", "window_id":index + 100}).to_string();
        if records {
            runtime.note_surface_execution("Computer", &input, Some(SurfaceExecution {
                surface_id: format!("fixture-surface-{index}"), state_revision:0,
                effect:SurfaceEffect::Observation,
            }), false, &mut batch);
        }
        runtime.note_tool_repetition("Computer", &input, false);
    }
    let admitted = runtime.tool_repetition_preflight("Computer", &observe(0).input, &mut batch).is_none();
    eprintln!("SURFACE_BOUND_METRIC {}", json!({"distinctCalls":600,"hostRecords":records,"newObservationAdmitted":admitted}));
    assert!(admitted, "600 distinct calls must not refuse a first observation; metadata={records}");
}

#[test]
fn unrecorded_browser_reads_keep_their_original_revision_and_description_keys() {
    let mut calls = Vec::new();
    for revision in 0..6 {
        if revision > 0 { calls.push(PlannedCall {
            name:"Bash".into(), input:json!({"command":format!("zerocode-browser goto browser-1 https://example.test/page-{revision} --workspace-revision {revision}")}).to_string(),
            result:Ok("legacy navigation".to_string().into()),
        }); }
        calls.push(PlannedCall { name:"Bash".into(),
            input:json!({"command":format!("zerocode-browser read browser-1 --workspace-revision {revision}"),
                         "description":format!("read page {revision}")}).to_string(),
            result:Ok("legacy browser result".to_string().into()),
        });
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime.run_turn("Read after each navigation on an older host", None).unwrap();
    let state = executor.state.lock().unwrap();
    let reads = state.executed.iter().filter(|call| call.name == "zerocode-browser"
        && serde_json::from_str::<Vec<String>>(&call.input).is_ok_and(|argv| argv.first().map(String::as_str) == Some("read"))).count();
    assert_eq!(reads, 6, "all six page reads, including the fourth after navigation, must retain the older host's main-compatible keys");
}

#[test]
fn verification_is_spent_only_for_an_armed_stop_and_preserves_the_stop_budget() {
    use super::repetition::ToolBatchRepetitionHardStops;
    let input = observe(7).input;
    let (mut runtime, _) = runtime_for(Vec::new());
    let mut batch = ToolBatchRepetitionHardStops::default();
    retain_script_result(&mut runtime, "Computer", &observe(7).input);
    runtime.note_surface_execution("Computer", &input, observe(7).result.unwrap().surface, false, &mut batch);
    let ack = computer("left_click", 41, 7, SurfaceEffect::Acknowledged);
    runtime.note_surface_execution(&ack.name, &ack.input, ack.result.unwrap().surface, false, &mut batch);
    assert!(runtime.tool_repetition_preflight("Computer", &input, &mut batch).is_none());
    for _ in 0..3 { runtime.note_tool_repetition("Computer", &input, false); }
    runtime.arm_tool_repetition_hard_stops();
    runtime.turn_progress_results = 1;
    let watermark = runtime.progress_marker_at_last_signal;
    let nudges = runtime.heuristic_stop_nudges;
    assert!(runtime.tool_repetition_preflight("Computer", &input, &mut batch).is_none(),
        "an unarmed preflight must leave the one pending verification available");
    assert_eq!(runtime.progress_marker_at_last_signal, watermark, "verification must not spend the progress watermark");
    assert_eq!(runtime.heuristic_stop_nudges, nudges, "verification must not spend a heuristic reprieve");
    assert!(runtime.tool_repetition_preflight("Computer", &input, &mut batch).is_some(),
        "the next same-state observation must see the real stop");
}

#[test]
fn editing_session_metadata_does_not_erase_surface_identity() {
    use super::repetition::ToolBatchRepetitionHardStops;
    let (mut runtime, _) = runtime_for(Vec::new());
    let mut batch = ToolBatchRepetitionHardStops::default();
    retain_script_result(&mut runtime, "Computer", &observe(7).input);
    let original = observe(7);
    runtime.note_surface_execution("Computer", &original.input, original.result.unwrap().surface, false, &mut batch);
    for _ in 0..3 { runtime.note_tool_repetition("Computer", &original.input, false); }
    runtime.arm_tool_repetition_hard_stops();
    let _session = runtime.session_mut();
    let alias = json!({"action":"observe", "app":"Fixture", "window_id":41, "ocr":false}).to_string();
    assert!(runtime.tool_repetition_preflight("Computer", &alias, &mut batch).is_some(),
        "session metadata access must preserve the previously observed target and its armed limit");
}

#[test]
fn distinct_invalid_capability_inputs_do_not_share_a_hard_stop() {
    let calls = (0..4).map(|index| PlannedCall {
        name:"CapabilityInvoke".into(),
        input:json!({"name":"Computer", "input":index, "nonce":index}).to_string(),
        result:Err(ToolError::new("invalid inner input")),
    }).collect();
    let (mut runtime, _) = runtime_for(calls);
    let summary = runtime.run_turn("Report four distinct invalid arguments", None).unwrap();
    assert_eq!(summary.iterations, 5, "unrelated invalid inner inputs must reach the natural model finish");
}

fn long_repetition_transcript() -> (ConversationRuntime<ScriptApi, ScriptExecutor>, ScriptExecutor) {
    use crate::session::{ContentBlock, ConversationMessage};
    let (mut runtime, executor) = runtime_for(Vec::new());
    for index in 0..2000 {
        let id = format!("long-transcript-{index}");
        let input = json!({"command":format!("printf fixture-{index}"), "description":"ordinary Bash call"}).to_string();
        runtime.session_mut().push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id:id.clone(), name:"Bash".into(), input,
        }])).unwrap();
        runtime.session_mut().push_message(ConversationMessage::tool_result(id, "Bash", "fixture result", false)).unwrap();
    }
    (runtime, executor)
}

#[test]
fn a_second_repetition_check_reuses_parsed_tool_use_fingerprints() {
    let (mut runtime, executor) = long_repetition_transcript();
    let input = json!({"command":"printf fixture-1999", "description":"ordinary Bash call"}).to_string();
    runtime.next_tool_repetition_hard_stop_notice("Bash", &input);
    runtime.session_mut().name = Some("Fixture metadata rename".into());
    let before = executor.resolutions.as_ref().unwrap().load(Ordering::Relaxed);
    runtime.next_tool_repetition_hard_stop_notice("Bash", &input);
    assert_eq!(executor.resolutions.as_ref().unwrap().load(Ordering::Relaxed) - before, 1,
        "the second check resolves its current call once and reuses all 2,000 transcript fingerprints");
}

// Measurement only: this emits timing samples, not a speed regression verdict.
// The resolution-count test above supplies the fallible cache assertion.
#[test]
fn measure_repetition_check_for_two_thousand_ordinary_tool_calls() {
    let (mut runtime, _) = long_repetition_transcript();
    runtime.tool_executor_mut().resolutions = None;
    let input = json!({"command":"printf fixture-1999", "description":"ordinary Bash call"}).to_string();
    runtime.next_tool_repetition_hard_stop_notice("Bash", &input);
    let mut samples = (0..101).map(|_| {
        let start = std::time::Instant::now();
        std::hint::black_box(runtime.next_tool_repetition_hard_stop_notice("Bash", &input));
        start.elapsed().as_nanos()
    }).collect::<Vec<_>>();
    samples.sort_unstable();
    eprintln!("REPETITION_CHECK_METRIC {}", json!({"toolCalls":2000,"samples":samples.len(),
        "p50Nanoseconds":samples[50],"p95Nanoseconds":samples[95],"debugAssertions":cfg!(debug_assertions)}));
}

fn retain_script_result(runtime: &mut ConversationRuntime<ScriptApi, ScriptExecutor>, name: &str, input: &str) {
    use crate::session::{ContentBlock, ConversationMessage};
    runtime.session_mut().push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id:"retained-surface-call".into(), name:name.into(), input:input.into(),
    }])).unwrap();
    runtime.session_mut().push_message(ConversationMessage::tool_result("retained-surface-call", name, "retained result", false)).unwrap();
}


#[test]
fn armed_verification_preserves_the_progress_watermark_and_reprieve() {
    use super::repetition::ToolBatchRepetitionHardStops;
    let (mut runtime, _) = runtime_for(Vec::new());
    let input = observe(7).input;
    retain_script_result(&mut runtime, "Computer", &input);
    let mut batch = ToolBatchRepetitionHardStops::default();
    runtime.note_surface_execution("Computer", &input, observe(7).result.unwrap().surface, false, &mut batch);
    for _ in 0..3 { runtime.note_tool_repetition("Computer", &input, false); }
    runtime.arm_tool_repetition_hard_stops();
    let ack = computer("left_click", 41, 7, SurfaceEffect::Acknowledged);
    runtime.note_surface_execution(&ack.name, &ack.input, ack.result.unwrap().surface, false, &mut batch);
    runtime.turn_progress_results = 1;
    let watermark = runtime.progress_marker_at_last_signal;
    let nudges = runtime.heuristic_stop_nudges;
    assert!(runtime.tool_repetition_preflight("Computer", &input, &mut batch).is_none());
    assert_eq!(runtime.progress_marker_at_last_signal, watermark, "acknowledged verification must not advance the progress watermark");
    assert_eq!(runtime.heuristic_stop_nudges, nudges, "acknowledged verification must not spend a heuristic reprieve");
}

#[test]
fn first_seen_computer_target_aliases_do_not_execute_past_an_attended_limit() {
    for (alias, multiple_windows, expected) in [
        (json!({"action":"observe","app":"Fixture"}), false, 3),
        (json!({"action":"observe","app":"pid:4242","window_id":41,"ocr":false}), false, 3),
        (json!({"action":"observe","app":"Fixture"}), true, 9),
    ] {
        let mut calls = vec![observe(7), observe(7), observe(7)];
        if multiple_windows {
            calls.extend((0..3).map(|_| computer("observe", 52, 7, SurfaceEffect::Observation)));
        }
        for _ in 0..6 {
            let mut call = if multiple_windows { computer("observe", 63, 0, SurfaceEffect::Observation) } else { observe(7) };
            call.input = alias.to_string();
            calls.push(call);
        }
        let (mut runtime, executor) = runtime_for(calls);
        runtime.set_attendance(Attendance::Attended);
        runtime.run_turn("Observe the target selected by this app-only call", None).unwrap();
        assert_eq!(executor.observations(), expected,
            "an app with multiple observed windows must let its host choose the target before applying that surface's limit: {alias}");
    }
}

// Guarding test: this protects runtime key separation; it also passes before this repair.
#[test]
fn a_new_window_of_the_same_app_is_not_the_previously_stopped_target() {
    let mut calls = vec![observe(7), observe(7), observe(7)];
    calls.push(computer("observe", 52, 0, SurfaceEffect::Observation));
    let (mut runtime, executor) = runtime_for(calls);
    runtime.set_attendance(Attendance::Attended);
    runtime.run_turn("Observe another authorized window of the same app", None).unwrap();
    assert_eq!(executor.observations(), 4, "an unknown explicit window must not borrow the app alias of another window");
}

// Guarding test: host view-revision tests supply the failing producer regression.
#[test]
fn alternating_unchanged_browser_views_each_stop_at_exactly_three_reads() {
    let mut calls = Vec::new();
    for _ in 0..6 {
        for selector in [None, Some("h1")] {
            let mut argv = vec!["read", "browser-1"];
            if let Some(selector) = selector { argv.push(selector); }
            calls.push(PlannedCall { name:"zerocode-browser".into(), input:serde_json::to_string(&argv).unwrap(),
                result:Ok(ToolExecutionResult { text:"unchanged view".into(), surface:Some(SurfaceExecution {
                    surface_id:"fixture-browser-surface".into(), state_revision:0, effect:SurfaceEffect::Observation,
                }) }) });
        }
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime.set_attendance(Attendance::Attended);
    runtime.run_turn("Read unchanged page and selector views without releasing either limit", None).unwrap();
    let state = executor.state.lock().unwrap();
    for length in [2, 3] {
        let count = state.executed.iter().filter(|call| serde_json::from_str::<Vec<String>>(&call.input)
            .is_ok_and(|argv| argv.len() == length)).count();
        assert_eq!(count, 3, "each unchanged browser view must stop at exactly three reads, argv length={length}");
    }
}

#[test]
fn a_first_seen_browser_selector_spelling_cannot_run_past_an_attended_limit() {
    let calls = (0..9).map(|index| PlannedCall { name:"zerocode-browser".into(),
        input:serde_json::to_string(&["read".to_string(), "browser-1".into(), format!("{}h1{}", " ".repeat(index), " ".repeat(index))]).unwrap(),
        result:Ok(ToolExecutionResult { text:"same heading".into(), surface:Some(SurfaceExecution {
            surface_id:"fixture-browser-surface".into(), state_revision:0, effect:SurfaceEffect::Observation,
        }) }),
    }).collect();
    let (mut runtime, executor) = runtime_for(calls);
    runtime.set_attendance(Attendance::Attended);
    runtime.run_turn("Keep whitespace aliases of the same selector bounded", None).unwrap();
    assert_eq!(executor.state.lock().unwrap().executed.len(), 3,
        "a first spelling of the same selector must not get a fourth execution");
}


#[test]
fn a_new_authenticated_control_identity_does_not_inherit_the_previous_identity_count() {
    let mut calls = vec![observe(0), observe(0)];
    for _ in 0..6 {
        let mut call = observe(0);
        call.result.as_mut().unwrap().surface.as_mut().unwrap().surface_id = "fixture-new-host-control".into();
        calls.push(call);
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime.set_attendance(Attendance::Attended);
    runtime.run_turn("Observe a new authenticated host control interval", None).unwrap();
    assert_eq!(executor.observations(), 5,
        "two old observations and exactly three new-control observations must keep separate counts");
}

#[test]
fn the_last_attested_target_identity_replaces_stale_exact_input_aliases() {
    let mut calls = vec![observe(0), observe(0), observe(0)];
    let mut action = computer("left_click", 41, 0, SurfaceEffect::Acknowledged);
    action.result.as_mut().unwrap().surface.as_mut().unwrap().surface_id = "fixture-new-host-control".into();
    calls.push(action);
    for _ in 0..6 {
        let mut call = observe(0);
        call.result.as_mut().unwrap().surface.as_mut().unwrap().surface_id = "fixture-new-host-control".into();
        calls.push(call);
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime.set_attendance(Attendance::Attended);
    runtime.run_turn("Use the latest host-attested identity for this target", None).unwrap();
    assert_eq!(executor.observations(), 6,
        "three observations on each authorized identity must use the target's latest host binding before dispatch");
}


#[test]
fn an_untrackable_new_control_uses_plain_counts_instead_of_a_stale_surface_limit() {
    use super::repetition::ToolBatchRepetitionHardStops;
    use zerocode_core::surface_execution::SURFACE_EXECUTION_CACHE_LIMIT;
    let (mut runtime, _) = runtime_for(Vec::new());
    let input = computer("observe", 100, 0, SurfaceEffect::Observation).input;
    retain_script_result(&mut runtime, "Computer", &input);
    let mut batch = ToolBatchRepetitionHardStops::default();
    for index in 0..SURFACE_EXECUTION_CACHE_LIMIT {
        let window = u64::try_from(index).unwrap() + 100;
        let call = computer("observe", window, 0, SurfaceEffect::Observation);
        runtime.note_surface_execution(&call.name, &call.input, call.result.unwrap().surface, false, &mut batch);
    }
    for _ in 0..3 { runtime.note_tool_repetition("Computer", &input, false); }
    runtime.arm_tool_repetition_hard_stops();
    let action = computer("left_click", 100, 0, SurfaceEffect::Acknowledged);
    runtime.note_surface_execution(&action.name, &action.input, Some(SurfaceExecution {
        surface_id:"fixture-new-host-control".into(), state_revision:0, effect:SurfaceEffect::Acknowledged,
    }), false, &mut batch);
    assert!(runtime.tool_repetition_preflight("Computer", &input, &mut batch).is_none(),
        "a valid new target record beyond the memory limit must use ordinary counts rather than a stale surface stop");
    for _ in 0..3 { runtime.note_tool_repetition("Computer", &input, false); }
    runtime.arm_tool_repetition_hard_stops();
    assert!(runtime.tool_repetition_preflight("Computer", &input, &mut batch).is_some(),
        "fallback must still stop the fourth ordinary identical call");
}


fn review_browser_call(command: &str, description: &str, effect: Option<SurfaceEffect>) -> PlannedCall {
    PlannedCall {
        name: "Bash".into(),
        input: json!({"command":command, "description":description}).to_string(),
        result: Ok(ToolExecutionResult {
            text: "fixture browser result".into(),
            surface: effect.map(|effect| SurfaceExecution {
                surface_id: "review-browser-surface".into(), state_revision: 0, effect,
            }),
        }),
    }
}

#[test]
fn review_attested_browser_marks_and_edit_screenshots_keep_main_counting() {
    let mut calls = vec![review_browser_call("zerocode-browser read browser-1", "initial read", Some(SurfaceEffect::Observation))];
    for index in 0..4 {
        calls.push(review_browser_call("zerocode-browser marks browser-1", &format!("marks after step {index}"), None));
    }
    for index in 0..4 {
        calls.push(PlannedCall { name: "edit_file".into(),
            input: json!({"path":"src/fixture.rs", "old":format!("old-{index}"), "new":format!("new-{index}")}).to_string(),
            result: Ok("edited fixture".to_string().into()),
        });
        calls.push(review_browser_call("zerocode-browser screenshot browser-1", "confirm the edit", None));
    }
    let (mut runtime, executor) = runtime_for(calls);
    runtime.run_turn("Use marks and edit/screenshot loops on an attested tab", None).unwrap();
    let state = executor.state.lock().unwrap();
    let count = state.executed.iter().filter(|call| call.name == "zerocode-browser"
        && serde_json::from_str::<Vec<String>>(&call.input).is_ok_and(|argv| matches!(argv.first().map(String::as_str), Some("marks" | "screenshot")))).count();
    assert_eq!(count, 8, "all four marks and four screenshots after successful edits must execute on an attested tab");
}

#[test]
fn review_browser_acknowledgements_keep_identical_legacy_actions_bounded() {
    let mut calls = vec![review_browser_call("zerocode-browser read browser-1", "initial read", Some(SurfaceEffect::Observation))];
    calls.extend((0..6).map(|_| review_browser_call("zerocode-browser click browser-1 --mark 1 --settle-later", "retry unchanged click", Some(SurfaceEffect::Acknowledged))));
    let (mut runtime, executor) = runtime_for(calls);
    runtime.set_attendance(Attendance::Attended);
    runtime.run_turn("Keep identical legacy action counts through host acknowledgements", None).unwrap();
    let count = executor.state.lock().unwrap().executed.iter().filter(|call| call.name == "zerocode-browser"
        && serde_json::from_str::<Vec<String>>(&call.input).is_ok_and(|argv| argv.first().map(String::as_str) == Some("click"))).count();
    assert_eq!(count, 3, "a browser acknowledgement must not migrate and erase its legacy action count");
}

#[test]
fn review_known_windows_of_one_app_keep_cached_transcript_fingerprints() {
    use super::repetition::ToolBatchRepetitionHardStops;
    let (mut runtime, executor) = long_repetition_transcript();
    let mut batch = ToolBatchRepetitionHardStops::default();
    let calls = [computer("observe", 41, 0, SurfaceEffect::Observation), computer("observe", 52, 0, SurfaceEffect::Observation)];
    for call in &calls {
        runtime.note_surface_execution(&call.name, &call.input, call.result.as_ref().unwrap().surface.clone(), false, &mut batch);
    }
    let input = json!({"command":"printf fixture-1999", "description":"ordinary Bash call"}).to_string();
    runtime.next_tool_repetition_hard_stop_notice("Bash", &input);
    for call in &calls {
        runtime.note_surface_execution(&call.name, &call.input, call.result.as_ref().unwrap().surface.clone(), false, &mut batch);
    }
    let before = executor.resolutions.as_ref().unwrap().load(Ordering::Relaxed);
    runtime.next_tool_repetition_hard_stop_notice("Bash", &input);
    assert_eq!(executor.resolutions.as_ref().unwrap().load(Ordering::Relaxed) - before, 1,
        "records from known windows of one app must reuse all 2,000 cached transcript fingerprints");
}


#[test]
fn review_one_host_chosen_app_window_bounds_new_input_spellings() {
    let calls = (0..6).map(|index| {
        let mut call = observe(7);
        call.input = json!({"action":"observe", "app":"Fixture", "ocr":index >= 3}).to_string();
        call
    }).collect();
    let (mut runtime, executor) = runtime_for(calls);
    runtime.set_attendance(Attendance::Attended);
    runtime.run_turn("Keep the sole host-chosen app window bounded across input spellings", None).unwrap();
    assert_eq!(executor.observations(), 3,
        "one host-chosen window must bind the app before a new input spelling gets a fourth observation");
}
