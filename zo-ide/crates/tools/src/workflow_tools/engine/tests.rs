use core_types::helper_run::HelperRun;
use super::*;

use super::super::worktree::WorktreeGuard;
use super::items::{extract_dedup_keys, semantic_verdict, seen_text};
use super::synthesis::parse_judgement;
use crate::workflow_tools::spec::WorkflowSpec;
use serde_json::json;
use std::sync::Arc;

// --- mock backend ------------------------------------------------------

/// What a mock spawn eventually reports for the agent it launches.
enum Outcome {
    Completed(String),
    /// Completed via the `StructuredOutput` tool: the runtime captured this
    /// value as `AgentCompletion.structured` (8c), independent of the text.
    Structured(Value),
    /// Completed and reported `output_tokens` — drives the output-token budget
    /// tests (the post-hoc spend the engine folds into `max_output_tokens`).
    CompletedTokens(String, u64),
    Failed(String),
    /// No completion recorded — `wait` reports `still_running`.
    Pending,
    /// The spawn call itself fails (agent never launches).
    SpawnError(String),
}

struct RecordedSpawn {
    description: String,
    prompt: String,
    subagent_type: Option<String>,
    model: Option<String>,
    allow_cross_provider: bool,
    route_model: Option<String>,
    route_source: Option<String>,
    cwd: Option<std::path::PathBuf>,
    prior_failures: u32,
}

/// Decides each mock spawn's eventual outcome from its call index + record.
type Responder = Box<dyn FnMut(usize, &RecordedSpawn) -> Outcome>;

struct MockBackend {
    next: usize,
    spawns: Vec<RecordedSpawn>,
    completions: HashMap<String, AgentCompletion>,
    responder: Responder,
    /// Per-output-token USD price the engine multiplies the token tally by for
    /// the `max_cost_usd` budget. `0.0` (default) leaves cost estimation off.
    price_per_token: f64,
    /// When true, simulate the live backend's timeout cleanup path by returning
    /// a terminal `stopped` completion for pending ids.
    cancel_pending: bool,
    /// The `error` the simulated cancel stamps. The legacy default is a
    /// neutral string (no retry marker), so pre-retry tests keep their exact
    /// behavior; `with_phase_timeout_cancel` switches to the live marker.
    cancel_error: &'static str,
    activity_template: Option<AgentActivitySnapshot>,
    usage_cost: Option<crate::misc_tools::AgentUsageCost>,
}

impl MockBackend {
    fn new(responder: impl FnMut(usize, &RecordedSpawn) -> Outcome + 'static) -> Self {
        Self {
            next: 0,
            spawns: Vec::new(),
            completions: HashMap::new(),
            responder: Box::new(responder),
            price_per_token: 0.0,
            cancel_pending: false,
            cancel_error: "mock timeout cancel",
            activity_template: None,
            usage_cost: None,
        }
    }

    /// Always-completes mock whose result is `prefix + index`.
    fn echo(prefix: &'static str) -> Self {
        Self::new(move |index, _| Outcome::Completed(format!("{prefix}{index}")))
    }

    /// Price each reported output token, enabling the `max_cost_usd` budget.
    fn with_price(mut self, price_per_token: f64) -> Self {
        self.price_per_token = price_per_token;
        self
    }

    fn with_timeout_cancel(mut self) -> Self {
        self.cancel_pending = true;
        self
    }

    /// Simulate the LIVE phase-timeout cancel exactly: terminal `stopped`
    /// carrying [`PHASE_TIMEOUT_STOP_ERROR`], which arms the timeout retry
    /// pass (unlike the neutral `with_timeout_cancel`).
    fn with_phase_timeout_cancel(mut self) -> Self {
        self.cancel_pending = true;
        self.cancel_error = PHASE_TIMEOUT_STOP_ERROR;
        self
    }

    fn with_startup_no_progress_cancel(mut self) -> Self {
        self.cancel_pending = true;
        self.cancel_error = STARTUP_NO_PROGRESS_STOP_ERROR;
        self
    }

    fn with_activity(mut self, activity: AgentActivitySnapshot) -> Self {
        self.activity_template = Some(activity);
        self
    }
}

impl AgentBackend for MockBackend {
    fn spawn(&mut self, input: AgentInput) -> Result<String, ToolError> {
        let record = RecordedSpawn {
            description: input.description,
            prompt: input.prompt,
            subagent_type: input.subagent_type,
            model: input.model,
            allow_cross_provider: input.allow_cross_provider,
            route_model: input.route_model,
            route_source: input.route_source,
            cwd: input.cwd,
            prior_failures: input.prior_failures,
        };
        let index = self.next;
        self.next += 1;
        let outcome = (self.responder)(index, &record);
        self.spawns.push(record);
        let id = format!("mock-agent-{index}");
        match outcome {
            Outcome::SpawnError(message) => Err(ToolError::Execution(message)),
            other => {
                if let Some(completion) = outcome_to_completion(&id, other) {
                    self.completions.insert(id.clone(), completion);
                }
                Ok(id)
            }
        }
    }

    fn wait(&self, ids: &[String], _timeout: Duration) -> Vec<AgentCompletion> {
        ids.iter()
            .map(|id| {
                self.completions
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| still_running(id))
            })
            .collect()
    }

    fn cancel(&self, id: &str) -> Option<AgentCompletion> {
        self.cancel_pending.then(|| AgentCompletion {
            agent_id: id.to_string(),
            name: id.to_string(),
            status: STATUS_STOPPED.to_string(),
            result: None,
            structured: None,
            error: Some(self.cancel_error.to_string()),
            run: HelperRun::default(),
        })
    }

    fn activity(&self, _id: &str) -> Option<AgentActivitySnapshot> {
        self.activity_template.clone()
    }

    fn usage_cost(&self, _id: &str) -> Option<crate::misc_tools::AgentUsageCost> {
        self.usage_cost.clone()
    }

    fn output_price_per_token(&self) -> f64 {
        self.price_per_token
    }
}

fn outcome_to_completion(id: &str, outcome: Outcome) -> Option<AgentCompletion> {
    let (status, result, structured, error, output_tokens) = match outcome {
        Outcome::Completed(text) => (STATUS_COMPLETED, Some(text), None, None, 0),
        Outcome::CompletedTokens(text, tokens) => {
            (STATUS_COMPLETED, Some(text), None, None, tokens)
        }
        // 8c: simulate an agent that returned via the `StructuredOutput`
        // tool — the runtime captured the call input directly. The text
        // result is deliberately non-JSON so a passing test proves the
        // captured value (not prose parsing) was used.
        Outcome::Structured(value) => (
            STATUS_COMPLETED,
            Some("(returned via StructuredOutput tool)".to_string()),
            Some(value),
            None,
            0,
        ),
        Outcome::Failed(message) => (STATUS_FAILED, None, None, Some(message), 0),
        Outcome::Pending | Outcome::SpawnError(_) => return None,
    };
    Some(AgentCompletion {
        agent_id: id.to_string(),
        name: id.to_string(),
        status: status.to_string(),
        result,
        structured,
        error,
        run: HelperRun {
            output_tokens,
            ..HelperRun::default()
        },
    })
}

fn still_running(id: &str) -> AgentCompletion {
    AgentCompletion {
        agent_id: id.to_string(),
        name: String::new(),
        status: STATUS_STILL_RUNNING.to_string(),
        result: None,
        structured: None,
        error: None,
        run: HelperRun::default(),
    }
}

fn delegated_task_prompt(prompt: &str) -> &str {
    prompt
        .split_once("\n\n[Workflow execution]")
        .map_or(prompt, |(task, _)| task)
}

// --- helpers -----------------------------------------------------------

fn workflow(value: &Value) -> NormalizedWorkflow {
    WorkflowSpec::from_value(value)
        .and_then(WorkflowSpec::validate)
        .expect("test workflow should be valid")
}

fn fast_opts() -> RunOptions<'static> {
    RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: None,
        cache: None,
        semantic_cache: None,
        worktree: None,
        progress: None,
        check: None,
    }
}

// --- worktree isolation mock (no real git; temp dirs) ------------------

/// In-process [`WorktreeProvider`] for tests: each `create` makes a temp
/// dir whose guard removes it on drop, and hands the guard a synthetic patch
/// so merge-back orchestration is exercisable without real git. Interior
/// mutability via `AtomicUsize`/`Mutex` so the provider is `Send + Sync` (the
/// trait now requires it, since a still-live worker's worktree can be handed to
/// a background cleanup owner).
struct TempWorktreeProvider {
    root: std::path::PathBuf,
    counter: std::sync::atomic::AtomicUsize,
    /// Patches handed to `apply_patch` that did not match `fail_substr`.
    applied: std::sync::Mutex<Vec<String>>,
    /// `apply_patch` errs when the patch contains this marker (conflict sim).
    fail_substr: Option<String>,
}

impl TempWorktreeProvider {
    fn new() -> Self {
        Self::with_conflict(None)
    }

    fn with_conflict(fail_substr: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(format!(
            "zo-wf-test-wt-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        Self {
            root,
            counter: std::sync::atomic::AtomicUsize::new(0),
            applied: std::sync::Mutex::new(Vec::new()),
            fail_substr: fail_substr.map(str::to_string),
        }
    }
}

impl WorktreeProvider for TempWorktreeProvider {
    fn create(&self, label: &str) -> Result<Box<dyn WorktreeGuard>, String> {
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = self.root.join(format!("{label}-{n}"));
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(Box::new(TempGuard {
            path,
            patch: Some(format!("PATCH:{label}-{n}")),
        }))
    }

    fn apply_patch(&self, patch: &str) -> Result<(), String> {
        if let Some(marker) = &self.fail_substr {
            if patch.contains(marker.as_str()) {
                return Err(format!("simulated conflict applying {patch}"));
            }
        }
        self.applied.lock().unwrap().push(patch.to_string());
        Ok(())
    }
}

struct TempGuard {
    path: std::path::PathBuf,
    /// Synthetic change-set this isolated dir reports; `None` mimics a clean
    /// tree (no merge-back).
    patch: Option<String>,
}

impl WorktreeGuard for TempGuard {
    fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn collect_patch(&self) -> Result<Option<String>, String> {
        Ok(self.patch.clone())
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn run_wf(
    wf: &NormalizedWorkflow,
    input: &Value,
    backend: &mut dyn AgentBackend,
) -> WorkflowReport {
    run(wf, input, backend, &fast_opts())
}

// --- tests -------------------------------------------------------------

#[test]
fn progress_events_track_phase_lifecycle() {
    use std::cell::RefCell;

    #[derive(Default)]
    struct RecordingSink {
        events: RefCell<Vec<String>>,
    }
    impl ProgressSink for RecordingSink {
        fn emit(&self, event: ProgressEvent<'_>) {
            let tag = match event {
                ProgressEvent::Started { phases, .. } => format!("started:{}", phases.len()),
                ProgressEvent::PhaseEnter { id, round } => format!("enter:{id}:{round}"),
                ProgressEvent::AgentsSpawned {
                    phase_id,
                    agent_ids,
                } => format!("spawned:{phase_id}:{}", agent_ids.len()),
                ProgressEvent::AgentDone {
                    phase_id, status, ..
                } => format!("agent_done:{phase_id}:{status}"),
                ProgressEvent::PhaseDone { id, completed, .. } => {
                    format!("done:{id}:{completed}")
                }
                ProgressEvent::PhaseResumed { id } => format!("resumed:{id}"),
                ProgressEvent::FindingQueued { phase_id, .. } => format!("finding:{phase_id}"),
                ProgressEvent::ItemCarried { phase_id, .. } => format!("carried:{phase_id}"),
                ProgressEvent::ItemInvalidated { phase_id, .. } => format!("invalidated:{phase_id}"),
                ProgressEvent::SelectiveRetryStarted { phase_id, .. } => format!("retry:{phase_id}"),
                ProgressEvent::FindingBlocked { phase_id, .. } => format!("blocked:{phase_id}"),
                ProgressEvent::SynthesizeEnter => "synth".to_string(),
                ProgressEvent::Finished { status } => format!("finished:{status}"),
            };
            self.events.borrow_mut().push(tag);
        }
    }

    // Two phases: a single agent, then one mapped over its result.
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "a", "prompt": "p1" },
            { "id": "b", "over": "a", "prompt": "p2" }
        ]
    }));
    let sink = RecordingSink::default();
    let opts = RunOptions {
        progress: Some(&sink),
        ..fast_opts()
    };
    let mut backend = MockBackend::echo("r");
    let report = run(&wf, &Value::Null, &mut backend, &opts);
    assert_eq!(report.status, "completed");

    let events = sink.events.borrow();
    // Skeleton emitted first, terminal state last.
    assert_eq!(events.first().map(String::as_str), Some("started:2"));
    assert_eq!(
        events.last().map(String::as_str),
        Some("finished:completed")
    );
    // Within a phase: enter → spawned → done; phase `a` fully precedes `b`.
    let a_enter = events
        .iter()
        .position(|e| e == "enter:a:1")
        .expect("a enter");
    let a_done = events.iter().position(|e| e == "done:a:1").expect("a done");
    let b_enter = events
        .iter()
        .position(|e| e == "enter:b:1")
        .expect("b enter");
    assert!(a_enter < a_done, "enter precedes done within a phase");
    assert!(a_done < b_enter, "phase a completes before phase b enters");
    assert!(
        events.iter().any(|e| e == "spawned:a:1"),
        "phase a spawned one agent"
    );
}

#[test]
fn single_phase_completes() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "only", "prompt": "do {input}" }]
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &json!("the-task"), &mut backend);

    assert_eq!(report.status, "completed");
    assert_eq!(report.agents_spawned, 1);
    assert_eq!(report.phases.len(), 1);
    assert_eq!(report.phases[0].items.len(), 1);
    assert_eq!(report.phases[0].items[0].status, STATUS_COMPLETED);
    assert_eq!(report.phases[0].items[0].result.as_deref(), Some("r0"));
    // `{input}` was substituted into the spawned prompt.
    assert_eq!(delegated_task_prompt(&backend.spawns[0].prompt), "do the-task");
}

#[test]
fn fanout_spawns_one_agent_per_item() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "review",
            "fanout": ["correctness", "security", "perf"],
            "prompt": "review {item} (#{index})",
            "subagent_type": "Explore"
        }]
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.agents_spawned, 3);
    assert_eq!(report.phases[0].items.len(), 3);
    assert_eq!(delegated_task_prompt(&backend.spawns[0].prompt), "review correctness (#0)");
    assert_eq!(delegated_task_prompt(&backend.spawns[2].prompt), "review perf (#2)");
    // subagent_type flows through to every spawn.
    assert_eq!(backend.spawns[1].subagent_type.as_deref(), Some("Explore"));
}

#[test]
fn phase_subagent_type_propagates_and_model_is_inherited() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "p", "prompt": "x",
            "subagent_type": "Verification"
        }]
    }));
    let mut backend = MockBackend::echo("r");
    run_wf(&wf, &Value::Null, &mut backend);
    assert_eq!(backend.spawns[0].model, None);
    assert!(!backend.spawns[0].allow_cross_provider);
    assert_eq!(
        backend.spawns[0].subagent_type.as_deref(),
        Some("Verification")
    );
}

#[test]
fn explicit_phase_model_allows_cross_provider_spawn() {
    let wf = workflow(&json!({
        "name": "explicit-model",
        "phases": [{
            "id": "repair",
            "prompt": "repair the findings",
            "model": "claude-fable-5"
        }]
    }));
    let mut backend = MockBackend::echo("r");
    run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(backend.spawns[0].model.as_deref(), Some("claude-fable-5"));
    assert!(backend.spawns[0].allow_cross_provider);
}

#[test]
fn dollar_input_fanout_expands_array() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["$input"], "prompt": "handle {item}" }]
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &json!(["alpha", "beta"]), &mut backend);

    assert_eq!(report.agents_spawned, 2);
    assert_eq!(delegated_task_prompt(&backend.spawns[0].prompt), "handle alpha");
    assert_eq!(delegated_task_prompt(&backend.spawns[1].prompt), "handle beta");
}

#[test]
fn over_maps_only_completed_prior_results() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "find", "fanout": ["a", "b"], "prompt": "find {item}" },
            { "id": "verify", "over": "find", "prompt": "verify:\n{item}" }
        ]
    }));
    // find: item 0 completes ("hit-a"), item 1 fails. verify should map the
    // one completed result only.
    let mut backend = MockBackend::new(|index, rec| match index {
        0 => Outcome::Completed("hit-a".to_string()),
        1 => Outcome::Failed("boom".to_string()),
        // verify phase: echo back the prompt it received.
        _ => Outcome::Completed(delegated_task_prompt(&rec.prompt).to_string()),
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    let verify = &report.phases[1];
    assert_eq!(
        verify.items.len(),
        1,
        "only the completed find item maps over"
    );
    assert_eq!(verify.items[0].input, "hit-a");
    assert_eq!(verify.items[0].result.as_deref(), Some("verify:\nhit-a"));
}

#[test]
fn failure_is_isolated() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["x", "y"], "prompt": "do {item}" }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Failed("nope".to_string()),
        _ => Outcome::Completed("ok".to_string()),
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(
        report.status, "completed",
        "one failure does not fail the run"
    );
    assert_eq!(report.phases[0].items[0].status, STATUS_FAILED);
    assert_eq!(report.phases[0].items[0].error.as_deref(), Some("nope"));
    assert_eq!(report.phases[0].items[1].status, STATUS_COMPLETED);
    assert!(report.notes.iter().any(|n| n.contains("1 agent(s) failed")));
}

#[test]
fn spawn_failure_becomes_failed_item() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["x"], "prompt": "do {item}" }]
    }));
    let mut backend = MockBackend::new(|_, _| Outcome::SpawnError("cannot launch".to_string()));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(
        report.agents_spawned, 0,
        "a failed spawn does not consume budget"
    );
    assert_eq!(report.phases[0].items[0].status, STATUS_FAILED);
    assert!(report.phases[0].items[0]
        .error
        .as_deref()
        .unwrap()
        .contains("cannot launch"));
}

#[test]
fn budget_clamps_fanout_and_marks_exhausted() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["a", "b", "c", "d", "e"], "prompt": "do {item}" }],
        "budget": { "max_agents": 2 }
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.agents_spawned, 2);
    assert!(report.budget_exhausted);
    assert_eq!(report.status, "budget_exhausted");
    assert_eq!(
        report.phases[0].items.len(),
        2,
        "fan-out was clamped to budget"
    );
}

#[test]
fn budget_skips_later_phases() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "first", "fanout": ["a", "b"], "prompt": "do {item}" },
            { "id": "second", "fanout": ["c"], "prompt": "do {item}" }
        ],
        "budget": { "max_agents": 2 }
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(
        report.phases.len(),
        1,
        "second phase skipped after exhaustion"
    );
    assert!(report.budget_exhausted);
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("later phases skipped")));
}

#[test]
fn output_tokens_are_summed_into_the_report_under_budget() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["a", "b"], "prompt": "do {item}" }],
        "budget": { "max_output_tokens": 1000 }
    }));
    // Two agents reporting 30 output tokens each → 60 spent, well under the cap.
    let mut backend = MockBackend::new(|_, _| Outcome::CompletedTokens("done".to_string(), 30));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(
        report.output_tokens, 60,
        "per-agent token reports are folded in"
    );
    assert!(!report.budget_exhausted);
    assert_eq!(report.status, "completed");
}

#[test]
fn output_token_budget_skips_later_phases_post_hoc() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "first", "fanout": ["a", "b"], "prompt": "do {item}" },
            { "id": "second", "fanout": ["c"], "prompt": "do {item}" }
        ],
        "budget": { "max_output_tokens": 150 }
    }));
    // The first phase's two agents report 100 tokens each → 200 spent, over the
    // 150 cap. The cap is post-hoc, so both first-phase agents still run (their
    // cost is only known after) and the *second* phase is what gets skipped.
    let mut backend = MockBackend::new(|_, _| Outcome::CompletedTokens("done".to_string(), 100));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(
        report.phases.len(),
        1,
        "second phase skipped once the token budget was blown"
    );
    assert_eq!(
        report.agents_spawned, 2,
        "only the first phase's agents ran"
    );
    assert_eq!(report.output_tokens, 200);
    assert!(report.budget_exhausted);
    assert_eq!(report.status, "budget_exhausted");
    assert!(
        report.notes.iter().any(|n| {
            n.contains("output-token budget exhausted") && n.contains("later phases skipped")
        }),
        "notes must name the token cap as the cause: {:?}",
        report.notes
    );
}

#[test]
fn per_phase_tokens_are_reported() {
    // WI-D: each phase carries the output tokens its own agents spent (the
    // run-cumulative delta), not just the grand total on the report.
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "first", "fanout": ["a", "b"], "prompt": "do {item}" },
            { "id": "second", "over": "first", "prompt": "verify {item}" }
        ]
    }));
    // First phase: 2 agents × 30 = 60. Second phase: 2 agents × 10 = 20.
    let mut backend = MockBackend::new(|index, _| {
        let tokens = if index < 2 { 30 } else { 10 };
        Outcome::CompletedTokens("done".to_string(), tokens)
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.phases.len(), 2);
    assert_eq!(
        report.phases[0].output_tokens, 60,
        "first phase token delta"
    );
    assert_eq!(
        report.phases[1].output_tokens, 20,
        "second phase token delta"
    );
    assert_eq!(
        report.output_tokens, 80,
        "grand total still sums the phases"
    );
    // Per-agent tokens are surfaced on each item too.
    assert!(report.phases[0].items.iter().all(|i| i.output_tokens == 30));
}

#[test]
fn remaining_tokens_surfaced() {
    // WI-D: a budgeted run reports the headroom left under `max_output_tokens`.
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["a", "b"], "prompt": "do {item}" }],
        "budget": { "max_output_tokens": 1000 }
    }));
    let mut backend = MockBackend::new(|_, _| Outcome::CompletedTokens("done".to_string(), 30));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.output_tokens, 60);
    assert_eq!(
        report.remaining_output_tokens,
        Some(940),
        "remaining = max - spent"
    );
}

#[test]
fn remaining_tokens_absent_without_a_budget() {
    // No budget → no remaining surface (the field stays omitted).
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["a"], "prompt": "do {item}" }]
    }));
    let mut backend = MockBackend::new(|_, _| Outcome::CompletedTokens("done".to_string(), 10));
    let report = run_wf(&wf, &Value::Null, &mut backend);
    assert_eq!(report.remaining_output_tokens, None);
    assert_eq!(report.remaining_cost_usd, None);
}

#[test]
fn rejected_request_does_not_block_a_successful_retry_under_budget() {
    let wf = workflow(&json!({
        "name": "retry-cost",
        "phases": [{"id": "a", "prompt": "first"}, {"id": "b", "prompt": "next"}],
        "budget": {"max_cost_usd": 1.0}
    }));
    let mut backend = MockBackend::echo("done");
    backend.usage_cost = Some(crate::misc_tools::AgentUsageCost {
        requests: 2, rejected_requests: 1, estimated_cost_usd: 0.1, ..Default::default()
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);
    assert_eq!(report.status, "completed");
    assert_eq!(report.agents_spawned, 2);
    assert!((report.remaining_cost_usd.unwrap() - 0.8).abs() < 1e-9);
    assert_eq!(report.usage_cost.unwrap().rejected_requests, 2);
}

#[test]
fn stopped_cost_receipt_is_labelled_as_a_snapshot() {
    let wf = workflow(&json!({"name": "stopped-cost", "phases": [{"id": "p", "prompt": "do"}]}));
    let mut backend = MockBackend::new(|_, _| Outcome::Pending).with_timeout_cancel();
    backend.usage_cost = Some(crate::misc_tools::AgentUsageCost {
        requests: 1, unreported_requests: 1, ..Default::default()
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);
    assert!(report.notes.iter().any(|note| note.contains("snapshot at logical stop")));
}

#[test]
fn cost_budget_is_rechecked_between_batches_of_one_phase() {
    let window = crate::misc_tools::workflow_concurrency_limit().max(1);
    let items: Vec<String> = (0..=window).map(|index| index.to_string()).collect();
    let wf = workflow(&json!({
        "name": "batch-cost", "phases": [{"id": "p", "fanout": items, "prompt": "do {item}"}],
        "budget": {"max_cost_usd": 0.01}
    }));
    let mut backend = MockBackend::echo("done");
    backend.usage_cost = Some(crate::misc_tools::AgentUsageCost {
        requests: 1, estimated_cost_usd: 0.1, ..Default::default()
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);
    assert_eq!(report.agents_spawned, window);
    assert_eq!(report.status, "budget_exhausted");
}

#[test]
fn incomplete_cost_stops_budgeted_work_without_claiming_headroom() {
    for cost in [
        None,
        Some(crate::misc_tools::AgentUsageCost { requests: 1, unreported_requests: 1, ..Default::default() }),
        Some(crate::misc_tools::AgentUsageCost { requests: 1, unpriced_requests: 1, ..Default::default() }),
    ] {
        let wf = workflow(&json!({
            "name": "incomplete-cost",
            "phases": [{"id": "a", "prompt": "first"}, {"id": "b", "prompt": "second"}],
            "budget": {"max_cost_usd": 1.0}
        }));
        let mut backend = MockBackend::echo("done");
        backend.usage_cost = cost;
        let report = run_wf(&wf, &Value::Null, &mut backend);
        assert_eq!(report.agents_spawned, 1);
        assert_eq!(report.status, "budget_exhausted");
        assert_eq!(report.remaining_cost_usd, None);
        assert!(report.notes.iter().any(|note| note.contains("cost budget blocked")));
    }
}

#[test]
fn request_usage_cost_overrides_parent_price_and_gates_next_phase() {
    let wf = workflow(&json!({
        "name": "metered",
        "phases": [
            {"id": "first", "fanout": ["a", "b"], "prompt": "do {item}"},
            {"id": "second", "prompt": "next"}
        ],
        "budget": {"max_cost_usd": 0.15}
    }));
    let mut backend = MockBackend::new(|_, _| Outcome::CompletedTokens("done".into(), 100))
        .with_price(100.0);
    backend.usage_cost = Some(crate::misc_tools::AgentUsageCost {
        requests: 2, unreported_requests: 1, unpriced_requests: 0, estimated_cost_usd: 0.1, ..Default::default()
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);
    assert_eq!(report.agents_spawned, 2);
    assert_eq!(report.phases.len(), 1);
    assert_eq!(report.status, "budget_exhausted");
    assert!((report.cost_usd - 0.2).abs() < 1e-9);
    assert_eq!(report.output_tokens, 200);
    let usage = report.usage_cost.as_ref().unwrap();
    assert_eq!(usage.requests, 4);
    assert_eq!(usage.unreported_requests, 2);
    assert!(report.notes.iter().any(|note| note.contains("not a billing receipt")));
}

#[test]
fn unknown_request_price_is_visible_without_parent_price_fallback() {
    let wf = workflow(&json!({
        "name": "unpriced", "phases": [{"id": "p", "prompt": "do"}]
    }));
    let mut backend = MockBackend::new(|_, _| Outcome::CompletedTokens("done".into(), 100))
        .with_price(100.0);
    backend.usage_cost = Some(crate::misc_tools::AgentUsageCost {
        requests: 1, unpriced_requests: 1, ..Default::default()
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);
    assert!(report.cost_usd.abs() < f64::EPSILON);
    assert_eq!(report.usage_cost.unwrap().unpriced_requests, 1);
    assert!(report.notes.iter().any(|note| note.contains("1 have unknown pricing")));
}

#[test]
fn cost_budget_exhaustion_stops_run() {
    // WI-D: a `max_cost_usd` cap stops the run post-hoc, exactly like the token
    // cap, deriving cost from the backend's per-token price.
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "first", "fanout": ["a", "b"], "prompt": "do {item}" },
            { "id": "second", "fanout": ["c"], "prompt": "do {item}" }
        ],
        "budget": { "max_cost_usd": 0.15 }
    }));
    // Each agent: 100 tokens × $0.001/token = $0.10. First phase = $0.20 > $0.15
    // cap → second phase skipped (post-hoc, both first-phase agents still ran).
    let mut backend = MockBackend::new(|_, _| Outcome::CompletedTokens("done".to_string(), 100))
        .with_price(0.001);
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.phases.len(), 1, "second phase skipped on cost");
    assert_eq!(report.agents_spawned, 2);
    assert!(report.budget_exhausted);
    assert_eq!(report.status, "budget_exhausted");
    assert!(
        (report.cost_usd - 0.20).abs() < 1e-9,
        "cost = tokens × price"
    );
    assert_eq!(report.remaining_cost_usd, Some(0.0), "cost budget is spent");
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("cost budget exhausted")),
        "notes must name the cost cap: {:?}",
        report.notes
    );
}

#[test]
fn output_token_budget_skips_synthesize() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["a", "b"], "prompt": "do {item}" }],
        "synthesize": { "prompt": "merge {all}" },
        "budget": { "max_output_tokens": 50 }
    }));
    // 80 spent across the phase (> 50) → the synthesize agent must not spawn.
    let mut backend = MockBackend::new(|_, _| Outcome::CompletedTokens("done".to_string(), 40));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert!(
        report.synthesis.is_none(),
        "synthesize must be budget-skipped"
    );
    assert_eq!(
        report.agents_spawned, 2,
        "no synthesize agent beyond the phase's two"
    );
    // The typed signal — not just the note — must report the cut-off: a skipped
    // deliverable is a budget exhaustion, never a clean `completed`.
    assert!(report.budget_exhausted);
    assert_eq!(report.status, "budget_exhausted");
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("synthesize skipped") && n.contains("output-token budget exhausted")));
}

#[test]
fn schema_extracts_json_first_try() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "p",
            "prompt": "audit",
            "schema": { "type": "object" }
        }]
    }));
    let mut backend =
        MockBackend::new(|_, _| Outcome::Completed("prose then {\"bugs\":3} tail".to_string()));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    let item = &report.phases[0].items[0];
    assert_eq!(item.structured, Some(json!({ "bugs": 3 })));
    assert_eq!(report.agents_spawned, 1, "no retry needed");
}

#[test]
fn schema_prefers_captured_structured_output_over_prose() {
    // 8c: when the agent answered via the `StructuredOutput` tool, the
    // engine uses the captured tool input exactly — even though the text
    // result is not JSON, no retry fires (the brittle prose path is only a
    // fallback).
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "p",
            "prompt": "audit",
            "schema": { "type": "object" }
        }]
    }));
    let mut backend =
        MockBackend::new(|_, _| Outcome::Structured(json!({ "verdict": "ok", "score": 9 })));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    let item = &report.phases[0].items[0];
    assert_eq!(
        item.structured,
        Some(json!({ "verdict": "ok", "score": 9 }))
    );
    assert_eq!(
        report.agents_spawned, 1,
        "captured tool output needs no schema retry"
    );
}

#[test]
fn schema_retries_then_succeeds() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "p",
            "prompt": "audit",
            "schema": { "type": "object" }
        }]
    }));
    // call 0 = prose (no JSON), call 1 (retry) = valid JSON.
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Completed("totally not json".to_string()),
        _ => Outcome::Completed("{\"ok\":true}".to_string()),
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    let item = &report.phases[0].items[0];
    assert_eq!(item.structured, Some(json!({ "ok": true })));
    assert_eq!(report.agents_spawned, 2, "one retry spawned");
}

#[test]
fn schema_failure_preserves_raw_result() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "p",
            "prompt": "audit",
            "schema": { "type": "object" }
        }]
    }));
    let mut backend = MockBackend::new(|_, _| Outcome::Completed("never json".to_string()));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    let item = &report.phases[0].items[0];
    assert!(item.structured.is_none(), "extraction failed both tries");
    assert_eq!(item.result.as_deref(), Some("never json"), "raw preserved");
    assert_eq!(report.agents_spawned, 2, "original + one retry");
}

#[test]
fn synthesize_runs_over_completed_items() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["a", "b"], "prompt": "do {item}" }],
        "synthesize": { "prompt": "combine:\n{all}", "model": "claude-fable-5" }
    }));
    let mut backend = MockBackend::new(|index, rec| {
        if rec.description.contains("synthesize") {
            Outcome::Completed(format!("SUMMARY<<{}>>", rec.prompt))
        } else {
            Outcome::Completed(format!("finding-{index}"))
        }
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.agents_spawned, 3, "2 fan-out + 1 synthesize");
    let synthesis = report.synthesis.expect("synthesis present");
    assert!(synthesis.contains("finding-0"));
    assert!(synthesis.contains("finding-1"));
    assert!(synthesis.starts_with("SUMMARY<<combine:"));
    let synth_spawn = backend
        .spawns
        .iter()
        .find(|spawn| spawn.description.contains("synthesize"))
        .expect("synthesize spawned");
    assert_eq!(synth_spawn.model.as_deref(), Some("claude-fable-5"));
    assert!(synth_spawn.allow_cross_provider);
}

#[test]
fn judge_selects_winner_from_candidates() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "gen", "fanout": ["a", "b"], "prompt": "do {item}" }],
        "judge": { "prompt": "pick best:\n{candidates}", "model": "claude-fable-5" }
    }));
    let mut backend = MockBackend::new(|index, rec| {
        if rec.description.contains("judge") {
            // Captured via StructuredOutput; text is deliberately non-JSON.
            Outcome::Structured(json!({
                "winner_index": 1,
                "rationale": "b is stronger",
                "ranking": [1, 0]
            }))
        } else {
            Outcome::Completed(format!("candidate-{index}"))
        }
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.agents_spawned, 3, "2 fan-out + 1 judge");
    let verdict = report.judgement.expect("judge verdict present");
    assert_eq!(verdict.winner_index, 1);
    assert_eq!(verdict.rationale, "b is stronger");
    assert_eq!(verdict.ranking, vec![1, 0]);
    // The judge prompt must carry the numbered candidate blocks.
    let judge_spawn = backend
        .spawns
        .iter()
        .find(|s| s.description.contains("judge"))
        .expect("judge spawned");
    assert!(judge_spawn.prompt.contains("### Candidate 0"));
    assert!(judge_spawn.prompt.contains("### Candidate 1"));
    assert_eq!(judge_spawn.model.as_deref(), Some("claude-fable-5"));
    assert!(judge_spawn.allow_cross_provider);
}

#[test]
fn judge_falls_back_to_parsing_json_from_text() {
    // An agent that ignored the StructuredOutput tool but emitted JSON still
    // yields a verdict via the extract_structured fallback.
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "gen", "fanout": ["a"], "prompt": "do {item}" }],
        "judge": { "prompt": "pick:\n{candidates}" }
    }));
    let mut backend = MockBackend::new(|_, rec| {
        if rec.description.contains("judge") {
            Outcome::Completed("here is my pick: {\"winner_index\": 0}".to_string())
        } else {
            Outcome::Completed("only-candidate".to_string())
        }
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);
    assert_eq!(report.judgement.expect("verdict").winner_index, 0);
}

#[test]
fn parse_judgement_reads_optional_fields() {
    let v = json!({ "winner_index": 2, "rationale": "best", "ranking": [2, 0, 1] });
    let j = parse_judgement(&v, 3).expect("valid verdict");
    assert_eq!(j.winner_index, 2);
    assert_eq!(j.rationale, "best");
    assert_eq!(j.ranking, vec![2, 0, 1]);

    let minimal = parse_judgement(&json!({ "winner_index": 0 }), 3).expect("minimal");
    assert_eq!(minimal.winner_index, 0);
    assert!(minimal.rationale.is_empty());
    assert!(minimal.ranking.is_empty());
}

#[test]
fn parse_judgement_rejects_bad_verdicts() {
    // Out-of-range winner (hallucinated ordinal) yields no verdict.
    assert!(parse_judgement(&json!({ "winner_index": 5 }), 3).is_none());
    // Missing winner_index.
    assert!(parse_judgement(&json!({ "rationale": "x" }), 3).is_none());
    // Not an object.
    assert!(parse_judgement(&json!([0, 1]), 3).is_none());
}

#[test]
fn still_running_when_no_completion_arrives() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "prompt": "slow" }]
    }));
    let mut backend = MockBackend::new(|_, _| Outcome::Pending);
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.phases[0].items[0].status, STATUS_STILL_RUNNING);
    assert!(report.notes.iter().any(|n| n.contains("still_running")));
}

#[test]
fn timeout_cancel_backend_marks_straggler_stopped() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "prompt": "slow" }]
    }));
    let mut backend = MockBackend::new(|_, _| Outcome::Pending).with_timeout_cancel();
    let report = run_wf(&wf, &Value::Null, &mut backend);

    let item = &report.phases[0].items[0];
    assert_eq!(item.status, STATUS_STOPPED);
    assert_eq!(item.error.as_deref(), Some("mock timeout cancel"));
    assert!(report.notes.iter().any(|n| n.contains("marked stopped")));
    // A neutral (non-phase-timeout) cancel must NOT arm the retry pass.
    let spawned: usize = 1;
    assert_eq!(report.agents_spawned, spawned);
}

/// The live failure this pins down: a plan agent force-stopped at the phase
/// timeout used to fail the phase outright and cancel every dependent phase.
/// The engine now retries the timed-out item once — the retry prompt carries
/// the salvage instruction, `prior_failures: 1` records the failed attempt for
/// policy/telemetry (promotion starts at two), and a recovered item reads
/// `completed`.
#[test]
fn phase_timeout_straggler_is_retried_once_and_recovers() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "plan", "prompt": "analyze everything" }]
    }));
    let mut backend = MockBackend::new(|index, _| {
        if index == 0 {
            Outcome::Pending
        } else {
            Outcome::Completed("retry finished the plan".to_string())
        }
    })
    .with_phase_timeout_cancel();
    let report = run_wf(&wf, &Value::Null, &mut backend);

    let item = &report.phases[0].items[0];
    assert_eq!(item.status, STATUS_COMPLETED, "retry recovers the item");
    assert_eq!(item.result.as_deref(), Some("retry finished the plan"));
    assert!(item.error.is_none(), "a recovered item carries no error");
    assert_eq!(report.agents_spawned, 2, "exactly one retry spawn");
    let retry = &backend.spawns[1];
    assert!(
        retry.prompt.contains("[Timeout retry]"),
        "retry prompt carries the timeout preamble: {}",
        retry.prompt
    );
    assert_eq!(
        retry.prior_failures, 1,
        "the retry records one prior failure without pretending it promoted"
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("automatically retried once") && n.contains("1 recovered")),
        "the report notes the recovery honestly: {:?}",
        report.notes
    );
}

/// The bounded half of the contract: when the retry ALSO stalls out, the item
/// keeps its honest `stopped` status with both failures recorded, and no third
/// attempt is ever spawned — recovery is a single pass, never a loop.
#[test]
fn phase_timeout_retry_that_also_stalls_keeps_stopped_status() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "plan", "prompt": "analyze everything" }]
    }));
    let mut backend =
        MockBackend::new(|_, _| Outcome::Pending).with_phase_timeout_cancel();
    let report = run_wf(&wf, &Value::Null, &mut backend);

    let item = &report.phases[0].items[0];
    assert_eq!(item.status, STATUS_STOPPED);
    assert!(
        item.error
            .as_deref()
            .is_some_and(|e| e.contains("automatic retry also failed")),
        "both attempts' failure is recorded: {:?}",
        item.error
    );
    assert_eq!(
        report.agents_spawned, 2,
        "one retry only — a still-failing item never loops"
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("automatically retried once") && n.contains("0 recovered")),
        "the report is honest that the retry did not recover: {:?}",
        report.notes
    );
}

#[test]
fn startup_watchdog_distinguishes_transport_reasoning_and_task_progress() {
    let policy = StartupWatchdogPolicy {
        first_action_timeout: Duration::from_secs(240),
        reasoning_extension: Duration::from_secs(240),
    };
    let transport_only = AgentActivitySnapshot {
        started_at: Some(90),
        stream_open_at: Some(100),
        ..AgentActivitySnapshot::default()
    };
    assert_eq!(
        startup_watchdog_decision(&transport_only, 339, false, policy),
        StartupWatchdogDecision::Continue
    );
    assert_eq!(
        startup_watchdog_decision(&transport_only, 340, false, policy),
        StartupWatchdogDecision::Stop,
        "transport life/keep-alives alone never extend startup"
    );

    let reasoning = AgentActivitySnapshot {
        stream_open_at: Some(100),
        last_reasoning_at: Some(320),
        effective_effort: Some("xhigh".to_string()),
        ..AgentActivitySnapshot::default()
    };
    assert_eq!(
        startup_watchdog_decision(&reasoning, 340, false, policy),
        StartupWatchdogDecision::ExtendOnce
    );
    assert_eq!(
        startup_watchdog_decision(&reasoning, 579, true, policy),
        StartupWatchdogDecision::Continue
    );
    assert_eq!(
        startup_watchdog_decision(&reasoning, 580, true, policy),
        StartupWatchdogDecision::Stop,
        "reasoning earns exactly one extension"
    );

    let acted = AgentActivitySnapshot {
        stream_open_at: Some(100),
        first_task_action_at: Some(120),
        ..AgentActivitySnapshot::default()
    };
    assert_eq!(
        startup_watchdog_decision(&acted, 10_000, false, policy),
        StartupWatchdogDecision::Continue,
        "the 20-minute phase cap, not startup watchdog, owns active work"
    );
}

#[test]
fn phase_inactivity_resets_only_on_task_progress() {
    let timeout = Duration::from_secs(1_200);
    let active = AgentActivitySnapshot {
        started_at: Some(90),
        stream_open_at: Some(100),
        last_reasoning_at: Some(2_190),
        first_task_action_at: Some(120),
        last_task_progress_at: Some(1_000),
        ..AgentActivitySnapshot::default()
    };
    assert!(!phase_inactivity_exceeded(&active, 100, 2_199, timeout));
    assert!(
        phase_inactivity_exceeded(&active, 100, 2_200, timeout),
        "transport and reasoning activity do not reset task inactivity"
    );

    let progressed = AgentActivitySnapshot {
        last_task_progress_at: Some(2_195),
        ..active.clone()
    };
    assert!(
        !phase_inactivity_exceeded(&progressed, 100, 2_200, timeout),
        "a recent tool/output progress event resets the inactivity window"
    );

    let running_tool = AgentActivitySnapshot {
        current_tool: Some("bash".to_string()),
        ..active.clone()
    };
    assert!(
        !phase_inactivity_exceeded(&running_tool, 100, 10_000, timeout),
        "a silent long-running tool is bounded by the hard cap, not inactivity"
    );

    let stale_manifest = AgentActivitySnapshot {
        started_at: Some(1),
        last_task_progress_at: Some(50),
        ..AgentActivitySnapshot::default()
    };
    assert!(
        !phase_inactivity_exceeded(&stale_manifest, 2_000, 2_100, timeout),
        "the barrier start floors stale manifest timestamps"
    );
}

#[test]
fn startup_no_progress_twice_uses_cross_provider_smart_fallback() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "implement", "prompt": "implement the change" }]
    }));
    let activity = AgentActivitySnapshot {
        selected_model: Some("gpt-5.6-sol".to_string()),
        fallback_models: vec![
            "gpt-5.6-terra".to_string(),
            "claude-sonnet-4-6".to_string(),
        ],
        ..AgentActivitySnapshot::default()
    };
    let mut backend = MockBackend::new(|index, _| match index {
        0 | 1 => Outcome::Pending,
        _ => Outcome::Completed("fallback completed".to_string()),
    })
    .with_startup_no_progress_cancel()
    .with_activity(activity);

    let report = run_wf(&wf, &Value::Null, &mut backend);
    let item = &report.phases[0].items[0];
    assert_eq!(item.status, STATUS_COMPLETED);
    assert_eq!(item.result.as_deref(), Some("fallback completed"));
    assert_eq!(report.agents_spawned, 3, "initial + retry + one fallback");
    assert_eq!(
        backend.spawns[1].route_model.as_deref(),
        Some("gpt-5.6-sol"),
        "the decisive retry stays on the same selected model"
    );
    let fallback = &backend.spawns[2];
    assert_eq!(fallback.prior_failures, 2);
    assert_eq!(fallback.route_model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(fallback.route_source.as_deref(), Some("fallback"));
    assert!(fallback.prompt.contains("[Startup provider fallback]"));
}

#[test]
fn startup_fallback_distinguishes_openai_compatible_provider_identity() {
    let activity = AgentActivitySnapshot {
        selected_model: Some("gpt-5.6-sol".to_string()),
        fallback_models: vec![
            "gpt-5.6-terra".to_string(),
            "deepseek-v4-pro".to_string(),
        ],
        ..AgentActivitySnapshot::default()
    };
    let inventory = runtime::ModelInventory::new(
        "gpt-5.6-sol",
        vec![
            runtime::ModelDescriptor::new("gpt-5.6-sol", "openai", "gpt"),
            runtime::ModelDescriptor::new("gpt-5.6-terra", "openai", "gpt"),
            runtime::ModelDescriptor::new("deepseek-v4-pro", "deepseek", "deepseek"),
        ],
    );

    let alternate = alternate_provider_route_in_inventory(&activity, &inventory)
        .expect("DeepSeek is a separate provider despite sharing OpenAI-compatible transport");
    assert_eq!(alternate.model, "deepseek-v4-pro");
    assert!(alternate.remaining_fallbacks.is_empty());
}

#[test]
fn startup_fallback_never_overrides_an_explicit_phase_model() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "implement",
            "model": "gpt-5.6-sol",
            "prompt": "implement the change"
        }]
    }));
    let activity = AgentActivitySnapshot {
        selected_model: Some("gpt-5.6-sol".to_string()),
        fallback_models: vec!["claude-sonnet-4-6".to_string()],
        ..AgentActivitySnapshot::default()
    };
    let mut backend = MockBackend::new(|_, _| Outcome::Pending)
        .with_startup_no_progress_cancel()
        .with_activity(activity);
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.agents_spawned, 2, "pin permits one retry but no reroute");
    assert!(backend.spawns.iter().all(|spawn| spawn.route_model.is_none()));
}

#[test]
fn workflow_prompt_starts_from_evidence_but_preserves_local_replanning() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "implement", "prompt": "do the work" }]
    }));
    let mut backend = MockBackend::echo("ok-");
    let _ = run_wf(&wf, &Value::Null, &mut backend);
    let prompt = &backend.spawns[0].prompt;
    assert!(prompt.contains("Start by checking the current files or other task evidence"));
    assert!(prompt.contains("make the smallest necessary local correction"));
    assert!(!prompt.contains("never replan"));
}

#[test]
fn downstream_phase_receives_only_actual_skill_tool_receipts() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "plan", "prompt": "make a plan" },
            { "id": "implement", "over": "plan", "prompt": "apply {item}" }
        ]
    }));
    let activity = AgentActivitySnapshot {
        loaded_skills: vec!["karpathy".to_string()],
        ..AgentActivitySnapshot::default()
    };
    let mut backend = MockBackend::new(|index, _| {
        Outcome::Completed(if index == 0 {
            "plan mentions another-skill only as prose".to_string()
        } else {
            "done".to_string()
        })
    })
    .with_activity(activity);
    let _ = run_wf(&wf, &Value::Null, &mut backend);

    assert!(!backend.spawns[0].prompt.contains("Workflow skill receipt"));
    let downstream = &backend.spawns[1].prompt;
    assert!(downstream.contains("actually loaded karpathy through the Skill tool"));
    assert!(!downstream.contains("actually loaded another-skill"));
}

#[test]
fn cancel_before_first_phase_spawns_nothing() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["a", "b"], "prompt": "do {item}" }]
    }));
    let flag = AtomicBool::new(true);
    let opts = RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: Some(&flag),
        cache: None,
        semantic_cache: None,
        worktree: None,
        progress: None,
        check: None,
    };
    let mut backend = MockBackend::echo("r");
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(report.status, "cancelled");
    assert_eq!(report.agents_spawned, 0);
    assert!(report.phases.is_empty());
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("cancelled before phase")));
}

#[test]
fn foreground_cancel_flag_wires_into_run_options() {
    // BUG-D6 wiring: round-trip the process-global flag, then prove it cancels a
    // run through the production `with_cancel` seam the TUI uses on Ctrl+C.
    clear_foreground_workflow_cancel();
    assert!(!foreground_workflow_cancel_flag().load(Ordering::SeqCst));
    request_foreground_workflow_cancel();
    assert!(foreground_workflow_cancel_flag().load(Ordering::SeqCst));

    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "p", "fanout": ["a", "b"], "prompt": "do {item}" }]
    }));
    let opts = RunOptions::production().with_cancel(foreground_workflow_cancel_flag());
    let mut backend = MockBackend::echo("r");
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(report.status, "cancelled");
    assert_eq!(report.agents_spawned, 0);

    // Leave the global clean for the production path / other tests.
    clear_foreground_workflow_cancel();
}

#[test]
fn cancel_between_phases_stops_after_current() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "first", "prompt": "a" },
            { "id": "second", "prompt": "b" }
        ]
    }));
    // Trip the flag while phase `first` spawns; phase `second`'s boundary
    // check then halts the run.
    let flag = Arc::new(AtomicBool::new(false));
    let trip = flag.clone();
    let mut backend = MockBackend::new(move |_, _| {
        trip.store(true, Ordering::Relaxed);
        Outcome::Completed("done".to_string())
    });
    let opts = RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: Some(&flag),
        cache: None,
        semantic_cache: None,
        worktree: None,
        progress: None,
        check: None,
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(report.status, "cancelled");
    assert_eq!(report.phases.len(), 1, "only the first phase ran");
    assert_eq!(report.phases[0].id, "first");
}

#[test]
fn worktree_isolation_without_provider_notes_fallback() {
    // isolation requested but no provider available (e.g. not a git repo):
    // run without isolation and say so — never silently pretend.
    let wf = workflow(&json!({
        "name": "demo",
        "isolation": "worktree",
        "phases": [{ "id": "p", "prompt": "x" }]
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &Value::Null, &mut backend);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("ran without isolation")),
        "notes: {:?}",
        report.notes
    );
    // No provider → agents keep the process-cwd default.
    assert!(backend.spawns[0].cwd.is_none());
}

#[test]
fn worktree_isolation_injects_per_agent_cwd() {
    // With a provider, every spawned agent gets its own isolated dir as cwd
    // and each dir is distinct (real per-item isolation, not a shared cwd).
    let wf = workflow(&json!({
        "name": "demo",
        "isolation": "worktree",
        "phases": [{ "id": "p", "fanout": ["a", "b"], "prompt": "do {item}" }]
    }));
    let mut backend = MockBackend::echo("r");
    let provider = TempWorktreeProvider::new();
    let opts = RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: None,
        cache: None,
        semantic_cache: None,
        worktree: Some(&provider),
        progress: None,
        check: None,
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(report.agents_spawned, 2);
    let cwds: Vec<_> = backend.spawns.iter().map(|s| s.cwd.clone()).collect();
    assert!(cwds.iter().all(Option::is_some), "every agent gets a cwd");
    assert_ne!(cwds[0], cwds[1], "each agent's worktree is distinct");
    for cwd in cwds.into_iter().flatten() {
        assert!(cwd.starts_with(std::env::temp_dir()));
    }
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("its own git worktree")),
        "notes: {:?}",
        report.notes
    );
}

#[test]
fn apply_none_isolates_but_does_not_merge_back() {
    // isolation:"worktree" without apply → agents are isolated but their
    // change-sets are discarded (current default; never silently applied).
    let wf = workflow(&json!({
        "name": "demo",
        "isolation": "worktree",
        "phases": [{ "id": "p", "fanout": ["a", "b"], "prompt": "edit {item}" }]
    }));
    let mut backend = MockBackend::echo("r");
    let provider = TempWorktreeProvider::new();
    let opts = RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: None,
        cache: None,
        semantic_cache: None,
        worktree: Some(&provider),
        progress: None,
        check: None,
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert!(
        provider.applied.lock().unwrap().is_empty(),
        "no patch should be applied without apply:\"sequential\""
    );
    assert!(
        report
            .notes
            .iter()
            .all(|n| !n.contains("back into the working tree")),
        "notes: {:?}",
        report.notes
    );
}

#[test]
fn apply_sequential_merges_each_agent_change_set() {
    // isolation:"worktree" + apply:"sequential" → every agent's collected
    // patch is applied back into the main tree, in spawn order.
    let wf = workflow(&json!({
        "name": "demo",
        "isolation": "worktree",
        "apply": "sequential",
        "phases": [{ "id": "edit", "fanout": ["a", "b", "c"], "prompt": "edit {item}" }]
    }));
    let mut backend = MockBackend::echo("r");
    let provider = TempWorktreeProvider::new();
    let opts = RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: None,
        cache: None,
        semantic_cache: None,
        worktree: Some(&provider),
        progress: None,
        check: None,
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(
        provider.applied.lock().unwrap().len(),
        3,
        "each agent's change-set merges back"
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("merged 3 agent change-set(s)")),
        "notes: {:?}",
        report.notes
    );
}

#[test]
fn fix_until_verified_repair_agents_honor_worktree_isolation_and_apply() {
    let wf = workflow(&json!({
        "name": "repair-isolation",
        "isolation": "worktree",
        "apply": "sequential",
        "phases": [{
            "id": "repair",
            "fanout": ["needs-fix"],
            "prompt": "validate {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id}", "model":"claude-fable-5"},
            "validator": {
                "prompt":"reverify {finding_id}",
                "model":"claude-fable-5",
                "schema":{"type":"object"}
            },
            "final_check": {"command":"cargo test"},
            "max_attempts": 1
        }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Structured(json!({
            "verdict":"fail",
            "title":"bug",
            "evidence":"broken",
            "affected_paths":["src/lib.rs"]
        })),
        1 => Outcome::Structured(json!({"changed_paths":["src/lib.rs"]})),
        2 => Outcome::Structured(json!({"verdict":"pass", "coverage":"reverified src/lib.rs"})),
        _ => Outcome::Completed("unexpected".to_string()),
    });
    let provider = TempWorktreeProvider::new();
    let opts = RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: None,
        cache: None,
        semantic_cache: None,
        worktree: Some(&provider),
        progress: None,
        check: Some(&|command| i32::from(command != "cargo test")),
    };

    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(report.status, "completed");
    assert_eq!(backend.spawns.len(), 3, "initial verifier + fixer + focused reverify");
    assert!(
        backend.spawns.iter().all(|spawn| spawn.cwd.is_some()),
        "every repair-loop agent must run in an isolated worktree: {:?}",
        backend.spawns.iter().map(|spawn| &spawn.cwd).collect::<Vec<_>>()
    );
    assert!(!backend.spawns[0].allow_cross_provider);
    assert!(
        backend.spawns[1..]
            .iter()
            .all(|spawn| spawn.allow_cross_provider),
        "explicit fixer and validator models may cross providers"
    );
    assert_eq!(
        provider.applied.lock().unwrap().len(),
        3,
        "repair-loop agents must use the same apply:\"sequential\" merge-back path"
    );
}

#[test]
fn apply_sequential_records_conflict_note_and_keeps_going() {
    // A change-set that fails to apply is recorded honestly; its siblings
    // still merge (no abort).
    let wf = workflow(&json!({
        "name": "demo",
        "isolation": "worktree",
        "apply": "sequential",
        "phases": [{ "id": "edit", "fanout": ["x", "y"], "prompt": "edit {item}" }]
    }));
    let mut backend = MockBackend::echo("r");
    // The first guard's patch is "PATCH:edit-0"; fail exactly that one.
    let provider = TempWorktreeProvider::with_conflict(Some("edit-0"));
    let opts = RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: None,
        cache: None,
        semantic_cache: None,
        worktree: Some(&provider),
        progress: None,
        check: None,
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(
        provider.applied.lock().unwrap().len(),
        1,
        "the non-conflicting sibling still merges"
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("did not merge cleanly") && n.contains("edit")),
        "notes: {:?}",
        report.notes
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("merged 1 agent change-set(s)")),
        "notes: {:?}",
        report.notes
    );
}

#[test]
fn command_green_stops_the_repeat_loop_when_the_check_passes() {
    // The TDD loop: implement → test → repeat-until-green. The check fails
    // round 1 and passes round 2, so exactly 2 rounds run.
    let wf = workflow(&json!({
        "name": "tdd",
        "phases": [{
            "id": "impl", "prompt": "fix {seen}",
            "repeat": { "max_rounds": 5, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));
    let mut backend = MockBackend::echo("r");
    let calls = std::cell::Cell::new(0u32);
    let check = |_cmd: &str| -> i32 {
        let n = calls.get();
        calls.set(n + 1);
        i32::from(n == 0) // 1 (fail) on round 1, 0 (green) thereafter
    };
    let opts = RunOptions {
        check: Some(&check),
        ..fast_opts()
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(report.phases[0].rounds, 2, "stopped the round after green");
    assert_eq!(calls.get(), 2, "check ran once per round until green");
    assert!(
        report.notes.iter().any(|n| n.contains("passed at round 2")),
        "notes: {:?}",
        report.notes
    );
}

#[test]
fn ordinary_command_green_repeat_escalates_only_after_completed_red_attempts() {
    let workflow_with_rounds = |max_rounds| {
        workflow(&json!({
            "name": "ordinary-quality-escalation",
            "phases": [{
                "id": "implement",
                "prompt": "implement the fix",
                "repeat": {
                    "max_rounds": max_rounds,
                    "until": {"command_green": {"command": "cargo test"}}
                }
            }]
        }))
    };
    let red = |_cmd: &str| 1;
    let opts = RunOptions {
        check: Some(&red),
        ..fast_opts()
    };

    let mut completed_red = MockBackend::echo("attempt-");
    run(
        &workflow_with_rounds(3),
        &Value::Null,
        &mut completed_red,
        &opts,
    );
    assert_eq!(
        completed_red
            .spawns
            .iter()
            .map(|spawn| spawn.prior_failures)
            .collect::<Vec<_>>(),
        vec![0, 1, 2],
        "the third ordinary implementation sees two completed-but-red attempts"
    );

    let mut provider_failure = MockBackend::new(|index, _| {
        if index == 0 {
            Outcome::Failed("429 rate limited".to_string())
        } else {
            Outcome::Completed("implementation finished".to_string())
        }
    });
    run(
        &workflow_with_rounds(2),
        &Value::Null,
        &mut provider_failure,
        &opts,
    );
    assert_eq!(
        provider_failure
            .spawns
            .iter()
            .map(|spawn| spawn.prior_failures)
            .collect::<Vec<_>>(),
        vec![0, 0],
        "a provider failure is not a completed red implementation"
    );

    let infra = |_cmd: &str| super::CHECK_INFRA_ERROR;
    let infra_opts = RunOptions {
        check: Some(&infra),
        ..fast_opts()
    };
    let mut checker_failure = MockBackend::echo("attempt-");
    run(
        &workflow_with_rounds(3),
        &Value::Null,
        &mut checker_failure,
        &infra_opts,
    );
    assert_eq!(
        checker_failure
            .spawns
            .iter()
            .map(|spawn| spawn.prior_failures)
            .collect::<Vec<_>>(),
        vec![0, 0, 0],
        "verification runner failures are not implementation quality failures"
    );

    let fanout = workflow(&json!({
        "name": "ordinary-fanout-quality-attribution",
        "phases": [{
            "id": "implement",
            "fanout": ["parser", "docs"],
            "prompt": "implement {item}",
            "repeat": {
                "max_rounds": 3,
                "until": {"command_green": {"command": "cargo test"}}
            }
        }]
    }));
    let mut fanout_red = MockBackend::echo("attempt-");
    run(&fanout, &Value::Null, &mut fanout_red, &opts);
    assert!(
        fanout_red
            .spawns
            .iter()
            .all(|spawn| spawn.prior_failures == 0),
        "a global red check cannot attribute failure to every fan-out item"
    );
}



#[test]
fn fix_until_verified_runs_fixer_reverify_and_final_check() {
    let wf = workflow(&json!({
        "name": "repair",
        "phases": [{
            "id": "verify",
            "fanout": ["case-a"],
            "prompt": "validate {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id} {finding} {evidence} {pass_receipts}"},
            "validator": {"prompt":"reverify {finding_id} {finding}", "schema":{"type":"object"}},
            "final_check": {"command":"cargo test -p demo"},
            "max_attempts": 2
        }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Structured(json!({"verdict":"fail", "title":"bug", "evidence":"broken", "affected_paths":["src/lib.rs"]})),
        1 => Outcome::Completed("fixed".to_string()),
        2 => Outcome::Structured(json!({"verdict":"pass", "coverage":"reverified src/lib.rs"})),
        _ => Outcome::Completed("unexpected".to_string()),
    });
    let opts = RunOptions {
        check: Some(&|command| i32::from(command != "cargo test -p demo")),
        ..fast_opts()
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);
    let phase = &report.phases[0];
    assert_eq!(report.status, "completed");
    assert_eq!(phase.findings.iter().filter(|finding| matches!(finding.state, FindingState::Fixed)).count(), 1);
    assert_eq!(phase.retried_finding_count, 1);
    assert_eq!(phase.blocked_finding_count, 0);
    assert!(phase.pass_receipts.iter().any(|receipt| receipt.coverage.contains("reverified")));
    assert_eq!(backend.spawns.len(), 3, "validator, fixer, focused reverify only");
    assert!(backend.spawns[1].prompt.contains("broken"));
    assert!(backend.spawns[2].prompt.contains("reverify"));
}

#[test]
fn fix_until_verified_invalidates_pass_receipts_for_changed_paths() {
    let wf = workflow(&json!({
        "name": "repair-invalidate",
        "phases": [{
            "id": "verify",
            "fanout": ["already-pass", "needs-fix"],
            "prompt": "validate {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id} with passes {pass_receipts}"},
            "validator": {"prompt":"reverify {finding_id}", "schema":{"type":"object"}},
            "final_check": {"command":"cargo test"},
            "max_attempts": 1
        }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Structured(json!({"verdict":"pass", "coverage":"initial src/lib.rs pass"})),
        1 => Outcome::Structured(json!({
            "verdict":"fail",
            "title":"bug",
            "evidence":"broken",
            "affected_paths":["src/main.rs"]
        })),
        2 => Outcome::Structured(json!({"changed_paths":["src/lib.rs"]})),
        3 => Outcome::Structured(json!({"verdict":"pass", "coverage":"reverified src/main.rs"})),
        _ => Outcome::Completed("unexpected".to_string()),
    });
    let opts = RunOptions {
        check: Some(&|command| i32::from(command != "cargo test")),
        ..fast_opts()
    };

    let report = run(&wf, &Value::Null, &mut backend, &opts);
    let receipts = &report.phases[0].pass_receipts;

    assert_eq!(report.status, "completed");
    assert!(receipts.iter().any(|receipt| receipt.coverage.contains("reverified src/main.rs")));
    assert!(
        receipts
            .iter()
            .all(|receipt| !receipt.coverage.contains("initial src/lib.rs pass")),
        "changed src/lib.rs must invalidate the stale initial receipt: {receipts:?}",
    );
}

#[test]
fn fix_until_verified_global_risk_fix_invalidates_non_overlapping_pass() {
    // Safety rule #6: a fix to a global-risk finding invalidates carried passes
    // broadly, even ones the fixer's changed paths do not overlap. The validator
    // declares the risk ("global"), so this is evidence-based, not a path list.
    let wf = workflow(&json!({
        "name": "repair-global-risk",
        "phases": [{
            "id": "verify",
            "fanout": ["already-pass", "needs-fix"],
            "prompt": "validate {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id}"},
            "validator": {"prompt":"reverify {finding_id}", "schema":{"type":"object"}},
            "final_check": {"command":"cargo test"},
            "max_attempts": 1
        }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Structured(json!({"verdict":"pass", "coverage":"initial src/lib.rs pass"})),
        1 => Outcome::Structured(json!({
            "verdict":"fail",
            "title":"scheduler bug",
            "evidence":"broken",
            "risk":"global",
            "affected_paths":["src/scheduler.rs"]
        })),
        // Fixer touches an UNRELATED path — no overlap with src/lib.rs.
        2 => Outcome::Structured(json!({"changed_paths":["src/other.rs"]})),
        3 => Outcome::Structured(json!({"verdict":"pass", "coverage":"reverified src/scheduler.rs"})),
        _ => Outcome::Completed("unexpected".to_string()),
    });
    let opts = RunOptions {
        check: Some(&|command| i32::from(command != "cargo test")),
        ..fast_opts()
    };

    let report = run(&wf, &Value::Null, &mut backend, &opts);
    let receipts = &report.phases[0].pass_receipts;

    assert_eq!(report.status, "completed");
    assert!(
        receipts.iter().all(|receipt| !receipt.coverage.contains("initial src/lib.rs pass")),
        "global-risk fix must broadly invalidate the non-overlapping pass: {receipts:?}",
    );
    assert!(receipts.iter().any(|receipt| receipt.coverage.contains("reverified src/scheduler.rs")));
}

#[test]
fn fix_until_verified_blocks_repeated_finding_and_final_red() {
    let wf = workflow(&json!({
        "name": "repair",
        "phases": [{
            "id": "verify",
            "fanout": ["case-a"],
            "prompt": "validate {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id}"},
            "validator": {"prompt":"reverify {finding_id}", "schema":{"type":"object"}},
            "final_check": {"command":"must-be-green"},
            "max_attempts": 1
        }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 | 2 => Outcome::Structured(json!({"verdict":"fail", "title":"same bug", "evidence":"still broken"})),
        1 => Outcome::Completed("attempted fix".to_string()),
        _ => Outcome::Completed("unexpected".to_string()),
    });
    let opts = RunOptions { check: Some(&|_| 1), ..fast_opts() };
    let report = run(&wf, &Value::Null, &mut backend, &opts);
    let phase = &report.phases[0];
    assert!(phase.blocked_finding_count >= 1);
    assert!(phase.findings.iter().any(|finding| matches!(finding.state, FindingState::Blocked)));
    assert!(phase.findings.iter().any(|finding| finding.id.starts_with("final-check:")));
}

#[test]
fn fix_until_verified_final_red_fails_the_run_status() {
    // The hard verification gate: a red `final_check` must decide the RUN status,
    // not just leave a blocked finding inside the phase report. Every agent here
    // completes and the validator passes, so the only red signal is the final
    // check — without the status gate the parent reads this run as a success.
    let wf = workflow(&json!({
        "name": "hard-gate",
        "phases": [{
            "id": "final_verification",
            "fanout": ["case-a"],
            "prompt": "verify {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id}"},
            "validator": {"prompt":"reverify {finding_id}", "schema":{"type":"object"}},
            "final_check": {"command":"cargo test"},
            "max_attempts": 1
        }]
    }));
    let mut backend = MockBackend::new(|_, _| {
        Outcome::Structured(json!({"verdict":"pass", "coverage":"checked everything"}))
    });
    let opts = RunOptions { check: Some(&|_| 1), ..fast_opts() };

    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(report.phases[0].blocked_finding_count, 1);
    assert_eq!(report.status, "failed");
    assert!(
        report.notes.iter().any(|note| note.contains("verification gate failed")),
        "the report must name the blocked gate: {:?}",
        report.notes
    );
}

#[test]
fn synthesis_prompt_carries_blocked_findings() {
    // The synthesizer is instructed to report a failure when a gate stays red, so
    // `{all}` must actually carry the blocked finding: completed-item text alone
    // hides it (the items all succeeded — the CHECK is what failed).
    let wf = workflow(&json!({
        "name": "hard-gate-synthesis",
        "phases": [{
            "id": "final_verification",
            "fanout": ["case-a"],
            "prompt": "verify {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id}"},
            "validator": {"prompt":"reverify {finding_id}", "schema":{"type":"object"}},
            "final_check": {"command":"cargo test"},
            "max_attempts": 1
        }],
        "synthesize": { "prompt": "summarize:\n{all}" }
    }));
    let mut backend = MockBackend::new(|_, record| {
        if record.description.contains("synthesize") {
            Outcome::Completed("summary".to_string())
        } else {
            Outcome::Structured(json!({"verdict":"pass", "coverage":"checked everything"}))
        }
    });
    let opts = RunOptions { check: Some(&|_| 1), ..fast_opts() };

    let _ = run(&wf, &Value::Null, &mut backend, &opts);

    let synthesis_prompt = backend
        .spawns
        .iter()
        .find(|spawn| spawn.description.contains("synthesize"))
        .map(|spawn| spawn.prompt.clone())
        .expect("synthesize agent should spawn");
    assert!(
        synthesis_prompt.contains("finding:blocked"),
        "synthesis input must label the blocked finding: {synthesis_prompt}"
    );
    assert!(
        synthesis_prompt.contains("final check failed"),
        "synthesis input must carry the blocked finding's title: {synthesis_prompt}"
    );
}

#[test]
fn fix_until_verified_same_evidence_blocks_before_attempt_cap() {
    // Isolation of "same-evidence repeated failure detection" (Phase 6) from the
    // attempt cap: with a generous max_attempts, an identical reverify finding
    // must block after the FIRST reverify by evidence match, not burn the cap.
    // `Finding::id` differs across reverify indices, so this only passes when the
    // block keys on evidence content (title/evidence/affected_paths).
    let wf = workflow(&json!({
        "name": "repair",
        "phases": [{
            "id": "verify",
            "fanout": ["case-a"],
            "prompt": "validate {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id}"},
            "validator": {"prompt":"reverify {finding_id}", "schema":{"type":"object"}},
            "final_check": {"command":"green"},
            "max_attempts": 5
        }]
    }));
    let same = json!({
        "verdict":"fail",
        "title":"same bug",
        "evidence":"still broken",
        "affected_paths":["src/lib.rs"]
    });
    let mut backend = MockBackend::new(move |index, _| match index {
        0 | 2 => Outcome::Structured(same.clone()),
        1 => Outcome::Completed("attempted fix".to_string()),
        _ => Outcome::Completed("unexpected".to_string()),
    });
    // Final check is green: the only block path under test is same-evidence.
    let opts = RunOptions { check: Some(&|command| i32::from(command != "green")), ..fast_opts() };
    let report = run(&wf, &Value::Null, &mut backend, &opts);
    let phase = &report.phases[0];
    assert!(phase.blocked_finding_count >= 1);
    assert!(phase.findings.iter().any(|finding| matches!(finding.state, FindingState::Blocked)));
    assert_eq!(
        backend.spawns.len(),
        3,
        "validator + one fixer + one reverify; same-evidence must block at attempt 1, not run the max_attempts=5 cap",
    );
}

#[test]
fn fix_until_verified_three_validators_one_failing_spawns_one_fixer_one_reverify() {
    // Phase 3 literal exit criterion: a three-validator fanout where only one
    // validator fails spawns exactly one fixer and one focused reverify — not all
    // three validators again. Total spawns = 3 initial + 1 fixer + 1 reverify = 5.
    let wf = workflow(&json!({
        "name": "repair",
        "phases": [{
            "id": "verify",
            "fanout": ["spec", "security", "regression"],
            "prompt": "validate {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id} {evidence}"},
            "validator": {"prompt":"reverify {finding_id}", "schema":{"type":"object"}},
            "final_check": {"command":"cargo test -p demo"},
            "max_attempts": 2
        }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Structured(json!({"verdict":"pass", "coverage":"spec checked"})),
        1 => Outcome::Structured(json!({"verdict":"pass", "coverage":"security checked"})),
        2 => Outcome::Structured(json!({
            "verdict":"fail", "title":"regression bug", "evidence":"failing case", "affected_paths":["src/x.rs"]
        })),
        3 => Outcome::Completed("patched".to_string()),
        4 => Outcome::Structured(json!({"verdict":"pass", "coverage":"regression reverified"})),
        other => panic!("unexpected spawn {other}"),
    });
    let opts = RunOptions {
        check: Some(&|command| i32::from(command != "cargo test -p demo")),
        ..fast_opts()
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);
    let phase = &report.phases[0];

    assert_eq!(
        backend.spawns.len(),
        5,
        "3 validators + 1 fixer + 1 reverify; passing validators are not respawned",
    );
    assert_eq!(report.status, "completed");
    assert_eq!(phase.carried_pass_count, 2, "spec + security carried, not respawned");
    assert_eq!(phase.retried_finding_count, 1, "only the regression finding is reverified");
    assert_eq!(phase.blocked_finding_count, 0);
    assert_eq!(
        phase.findings.iter().filter(|finding| matches!(finding.state, FindingState::Fixed)).count(),
        1,
    );
    assert!(backend.spawns[3].prompt.contains("failing case"), "fixer receives the finding evidence");
}

#[test]
fn fix_until_verified_emits_finding_carried_and_retry_progress_events() {
    // The repair loop must actually EMIT the selective-loop progress events the
    // TUI renders (a finding queued, an unaffected pass carried, a selective
    // retry started) — not just populate report fields.
    #[derive(Default)]
    struct RecordingSink {
        tags: std::cell::RefCell<Vec<String>>,
    }
    impl ProgressSink for RecordingSink {
        fn emit(&self, event: ProgressEvent<'_>) {
            let tag = match event {
                ProgressEvent::FindingQueued { .. } => "finding_queued",
                ProgressEvent::ItemCarried { .. } => "item_carried",
                ProgressEvent::SelectiveRetryStarted { .. } => "selective_retry",
                _ => return,
            };
            self.tags.borrow_mut().push(tag.to_string());
        }
    }

    let wf = workflow(&json!({
        "name": "repair",
        "phases": [{
            "id": "verify",
            "fanout": ["already-pass", "needs-fix"],
            "prompt": "validate {item}",
            "schema": {"type":"object"},
            "strategy": "fix_until_verified",
            "fixer": {"prompt":"fix {finding_id}"},
            "validator": {"prompt":"reverify {finding_id}", "schema":{"type":"object"}},
            "final_check": {"command":"cargo test"},
            "max_attempts": 2
        }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Structured(json!({"verdict":"pass", "coverage":"already covered"})),
        1 => Outcome::Structured(json!({"verdict":"fail", "title":"bug", "evidence":"broken", "affected_paths":["src/y.rs"]})),
        2 => Outcome::Completed("patched".to_string()),
        3 => Outcome::Structured(json!({"verdict":"pass", "coverage":"reverified"})),
        other => panic!("unexpected spawn {other}"),
    });
    let sink = RecordingSink::default();
    let opts = RunOptions {
        check: Some(&|command| i32::from(command != "cargo test")),
        progress: Some(&sink),
        ..fast_opts()
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);
    assert_eq!(report.status, "completed");

    let tags = sink.tags.borrow();
    assert!(tags.iter().any(|tag| tag == "finding_queued"), "must emit FindingQueued: {tags:?}");
    assert!(tags.iter().any(|tag| tag == "item_carried"), "must emit ItemCarried: {tags:?}");
    assert!(tags.iter().any(|tag| tag == "selective_retry"), "must emit SelectiveRetryStarted: {tags:?}");
}

#[test]
fn fix_until_verified_schema_is_backward_compatible() {
    let wf = workflow(&json!({
        "name": "old",
        "phases": [{"id":"p", "prompt":"do it"}]
    }));
    assert!(wf.phases[0].repair_loop.is_none());

    let wf = workflow(&json!({
        "name": "new",
        "phases": [{
            "id":"p",
            "prompt":"validate",
            "strategy":"fix_until_verified",
            "fixer":{"prompt":"fix {finding_id}"},
            "final_check":{"command":"true"}
        }]
    }));
    let repair = wf.phases[0].repair_loop.as_ref().expect("repair loop");
    assert_eq!(repair.max_attempts, 2);
}

#[test]
fn semantic_verdict_treats_completed_fail_as_finding() {
    let structured = json!({ "verdict": "fail", "finding_id": "F-1" });

    assert_eq!(semantic_verdict(STATUS_COMPLETED, Some(&structured)), Some("finding"));
}

#[test]
fn semantic_verdict_requires_coverage_for_generic_pass() {
    let pass_without_coverage = json!({ "verdict": "pass" });
    let pass_with_coverage = json!({ "verdict": "pass", "evidence": ["cargo test"] });

    assert_eq!(semantic_verdict(STATUS_COMPLETED, Some(&pass_without_coverage)), None);
    assert_eq!(semantic_verdict(STATUS_COMPLETED, Some(&pass_with_coverage)), Some("pass"));
}

#[test]
fn command_green_schema_repeat_carries_passes_and_retries_findings() {
    let wf = workflow(&json!({
        "name": "selective-repeat",
        "phases": [{
            "id": "verify",
            "fanout": ["passed item", "finding item"],
            "prompt": "verify {item} with {seen}",
            "schema": {"type": "object"},
            "repeat": { "max_rounds": 3, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        1 => Outcome::Structured(json!({"spec": false, "regression": true, "security": true, "issues": ["bug"]})),
        0 | 2 => Outcome::Structured(json!({"spec": true, "regression": true, "security": true})),
        other => panic!("unexpected spawn {other}"),
    });
    let calls = std::cell::Cell::new(0u32);
    let check = |_cmd: &str| -> i32 {
        let n = calls.get();
        calls.set(n + 1);
        i32::from(n == 0)
    };
    let opts = RunOptions {
        check: Some(&check),
        ..fast_opts()
    };

    let report = run(&wf, &Value::Null, &mut backend, &opts);
    let phase = &report.phases[0];

    assert_eq!(backend.spawns.len(), 3, "only the finding item is retried");
    assert_eq!(phase.rounds, 2);
    assert_eq!(phase.carried_pass_count, 1);
    assert_eq!(phase.retried_finding_count, 1);
    assert_eq!(phase.skipped_count, 1);
    assert!(phase.items.iter().any(|item| item.index == 0 && item.carried));
    assert!(phase.items.iter().any(|item| item.index == 1));
    assert!(
        phase.items.iter().any(|item| item.index == 0
            && item.carry_reason.as_deref() == Some("semantic pass carried into next repeat round")),
        "carried item must record a concrete carry reason: {:?}",
        phase.items,
    );
    assert_eq!(calls.get(), 2, "command_green remains the final gate");
}

#[test]
fn selective_repeat_counts_only_prior_quality_failures_for_model_escalation() {
    let workflow_with_rounds = |max_rounds| {
        workflow(&json!({
            "name": "quality-escalation",
            "phases": [{
                "id": "implement",
                "prompt": "implement and verify {item}",
                "schema": {"type": "object"},
                "repeat": {
                    "max_rounds": max_rounds,
                    "until": {"command_green": {"command": "cargo test"}},
                    "dedup_by": "finding_id"
                }
            }]
        }))
    };
    let check = |_cmd: &str| 1;
    let opts = RunOptions {
        check: Some(&check),
        ..fast_opts()
    };

    let mut quality_failures = MockBackend::new(|_, _| {
        Outcome::Structured(json!({"verdict": "fail", "finding_id": "F-1"}))
    });
    run(
        &workflow_with_rounds(3),
        &Value::Null,
        &mut quality_failures,
        &opts,
    );
    assert_eq!(
        quality_failures
            .spawns
            .iter()
            .map(|spawn| spawn.prior_failures)
            .collect::<Vec<_>>(),
        vec![0, 1, 2],
        "the third implementation attempt sees two prior semantic failures"
    );

    let mut provider_failure = MockBackend::new(|index, _| {
        if index == 0 {
            Outcome::Failed("429 rate limited".to_string())
        } else {
            Outcome::Structured(json!({"verdict": "fail", "finding_id": "F-1"}))
        }
    });
    run(
        &workflow_with_rounds(2),
        &Value::Null,
        &mut provider_failure,
        &opts,
    );
    assert_eq!(
        provider_failure
            .spawns
            .iter()
            .map(|spawn| spawn.prior_failures)
            .collect::<Vec<_>>(),
        vec![0, 0],
        "a provider/rate-limit failure must not unlock Sol or Fable"
    );

    let mut interrupted_failures = MockBackend::new(|index, _| match index {
        0 | 2 | 3 => Outcome::Structured(json!({"verdict": "fail", "finding_id": "F-1"})),
        1 => Outcome::Failed("429 rate limited".to_string()),
        other => panic!("unexpected spawn {other}"),
    });
    run(
        &workflow_with_rounds(4),
        &Value::Null,
        &mut interrupted_failures,
        &opts,
    );
    assert_eq!(
        interrupted_failures
            .spawns
            .iter()
            .map(|spawn| spawn.prior_failures)
            .collect::<Vec<_>>(),
        vec![0, 1, 1, 2],
        "429 pauses neither the quality ledger nor the two-real-failures threshold"
    );

    let mut invalid_output = MockBackend::new(|_, _| {
        Outcome::Completed("not structured JSON".to_string())
    });
    run(
        &workflow_with_rounds(2),
        &Value::Null,
        &mut invalid_output,
        &opts,
    );
    assert!(
        invalid_output
            .spawns
            .iter()
            .all(|spawn| spawn.prior_failures == 0),
        "schema-format retries are not implementation-quality failures"
    );
}

#[test]
fn auto_lane_prompt_orchestrates_single_phase_into_fanout() {
    let wf = workflow(&json!({
        "name": "auto-lanes",
        "phases": [{
            "id": "implement",
            "prompt": "Implement {item}. Split into parallel lanes: parser, executor, docs."
        }]
    }));
    let mut backend = MockBackend::echo("done-");

    let report = run(&wf, &Value::Null, &mut backend, &fast_opts());

    assert_eq!(report.agents_spawned, 3);
    let prompts: Vec<_> = backend.spawns.iter().map(|spawn| spawn.prompt.as_str()).collect();
    assert!(prompts.iter().any(|prompt| prompt.contains("Implement parser")));
    assert!(prompts.iter().any(|prompt| prompt.contains("Implement executor")));
    assert!(prompts.iter().any(|prompt| prompt.contains("Implement docs")));
}

#[test]
fn command_green_schema_repeat_requires_all_latest_semantic_passes() {
    let wf = workflow(&json!({
        "name": "invalid-never-greens",
        "phases": [{
            "id": "verify",
            "fanout": ["passed item", "invalid item"],
            "prompt": "verify {item}",
            "schema": {"type": "object"},
            "repeat": { "max_rounds": 2, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Structured(json!({"spec": true, "regression": true, "security": true})),
        _ => Outcome::Completed("not json, not a semantic pass".to_string()),
    });
    let calls = std::cell::Cell::new(0u32);
    let check = |_cmd: &str| -> i32 {
        calls.set(calls.get() + 1);
        0
    };
    let opts = RunOptions {
        check: Some(&check),
        ..fast_opts()
    };

    let report = run(&wf, &Value::Null, &mut backend, &opts);
    let phase = &report.phases[0];

    assert_eq!(phase.rounds, 2, "green command alone must not stop invalid verifier output");
    assert_eq!(phase.carried_pass_count, 1, "the valid pass is carried exactly once");
    assert_eq!(phase.skipped_count, 1, "only the pass is skipped on retry");
    assert_eq!(backend.spawns.len(), 5, "invalid item gets schema retry in both rounds");
    assert_eq!(calls.get(), 2, "the command still runs once per round");
    assert!(
        !report.notes.iter().any(|note| note.contains("passed at round")),
        "invalid semantic output must not be reported green: {:?}",
        report.notes
    );
}

#[test]
fn command_green_schema_repeat_uses_cross_run_semantic_pass_cache() {
    #[derive(Default)]
    struct MemorySemanticCache(std::cell::RefCell<std::collections::BTreeMap<SemanticCacheKey, PassReceipt>>);
    impl SemanticCache for MemorySemanticCache {
        fn load_pass(&self, key: &SemanticCacheKey) -> Option<PassReceipt> {
            self.0.borrow().get(key).cloned()
        }
        fn store_pass(&self, key: &SemanticCacheKey, receipt: &PassReceipt) {
            self.0.borrow_mut().insert(key.clone(), receipt.clone());
        }
    }

    let wf = workflow(&json!({
        "name": "semantic-cache",
        "phases": [{
            "id": "verify",
            "fanout": ["parser", "executor"],
            "prompt": "verify {item}",
            "schema": {"type": "object"},
            "repeat": { "max_rounds": 2, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));
    let cache = MemorySemanticCache::default();
    let check = |_cmd: &str| 0;
    let opts = RunOptions {
        semantic_cache: Some(&cache),
        check: Some(&check),
        ..fast_opts()
    };

    let mut first_backend = MockBackend::new(|index, spawn| {
        Outcome::Structured(json!({
            "verdict": "pass",
            "coverage": format!("{} covered", spawn.prompt),
            "receipt_key": format!("receipt-{index}"),
        }))
    });
    let first = run(&wf, &Value::Null, &mut first_backend, &opts);
    assert_eq!(first.status, "completed");
    assert_eq!(first_backend.spawns.len(), 2);

    let mut second_backend = MockBackend::new(|_, _| Outcome::Completed("should not spawn".to_string()));
    let second = run(&wf, &Value::Null, &mut second_backend, &opts);
    let phase = &second.phases[0];

    assert_eq!(second.status, "completed");
    assert_eq!(second_backend.spawns.len(), 0, "cached pass receipts avoid verifier respawn");
    assert_eq!(phase.carried_pass_count, 2);
    assert_eq!(phase.skipped_count, 2);
    assert!(phase
        .pass_receipts
        .iter()
        .any(|receipt| receipt.coverage.contains("parser")));

    let changed_prompt_wf = workflow(&json!({
        "name": "semantic-cache",
        "phases": [{
            "id": "verify",
            "fanout": ["parser", "executor"],
            "prompt": "verify formatting {item}",
            "schema": {"type": "object"},
            "repeat": { "max_rounds": 1, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));
    let mut changed_backend = MockBackend::new(|index, _| {
        Outcome::Structured(json!({"verdict": "pass", "coverage": format!("changed prompt {index}")}))
    });
    let changed = run(&changed_prompt_wf, &Value::Null, &mut changed_backend, &opts);
    assert_eq!(changed.status, "completed");
    assert_eq!(
        changed_backend.spawns.len(),
        2,
        "changing verifier semantics must miss the cross-run semantic cache"
    );
}

#[test]
fn command_green_schema_repeat_does_not_turn_carried_passes_into_success() {
    let wf = workflow(&json!({
        "name": "carried-red-check",
        "phases": [{
            "id": "verify",
            "fanout": ["a", "b"],
            "prompt": "verify {item}",
            "schema": {"type": "object"},
            "repeat": { "max_rounds": 2, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));
    let mut backend = MockBackend::new(|_, _| {
        Outcome::Structured(json!({"spec": true, "regression": true, "security": true}))
    });
    let check = |_cmd: &str| -> i32 { 1 };
    let opts = RunOptions {
        check: Some(&check),
        ..fast_opts()
    };

    let report = run(&wf, &Value::Null, &mut backend, &opts);
    let phase = &report.phases[0];

    assert_eq!(backend.spawns.len(), 2, "round two carries both passes without respawn");
    assert_eq!(phase.carried_pass_count, 2);
    assert_eq!(phase.skipped_count, 2);
    assert!(!report.notes.iter().any(|note| note.contains("passed at round")), "red command must not be reported green");
}

#[test]
fn phase_report_deserializes_without_selective_repeat_fields() {
    let value = json!({
        "id": "legacy",
        "rounds": 1,
        "items": [{
            "index": 0,
            "input": "x",
            "agent_id": "a0",
            "status": "completed",
            "result": "ok"
        }]
    });

    let phase: PhaseReport = serde_json::from_value(value).expect("legacy phase report");
    assert_eq!(phase.carried_pass_count, 0);
    assert_eq!(phase.retried_finding_count, 0);
    assert_eq!(phase.skipped_count, 0);
    assert!(!phase.items[0].carried);
    assert_eq!(phase.items[0].semantic_verdict, None);
}

#[test]
fn pipeline_route_modifies_worktree_and_verifies() {
    // The route an "implement this and make the tests pass" request should take:
    // plan → implement (isolated worktree, merged back into the working tree) →
    // verify (run the checks, repeat until green). Proves the implementation
    // pipeline produces a real change-set and actually runs its verification,
    // instead of returning analysis only (WI-R2 / success criterion S2).
    let wf = workflow(&json!({
        "name": "implement-and-verify",
        "isolation": "worktree",
        "apply": "sequential",
        "phases": [
            { "id": "plan", "prompt": "plan the change for: {input}" },
            { "id": "implement", "over": "plan", "prompt": "implement the approved plan: {item}" },
            {
                "id": "verify",
                "prompt": "run the required checks",
                "repeat": {
                    "max_rounds": 3,
                    "until": { "command_green": { "command": "cargo test --all-targets" } }
                }
            }
        ]
    }));
    let mut backend = MockBackend::echo("r");
    let provider = TempWorktreeProvider::new();
    let rounds = std::cell::Cell::new(0u32);
    let check = |_cmd: &str| -> i32 {
        let n = rounds.get();
        rounds.set(n + 1);
        i32::from(n == 0) // red on round 1, green on round 2 (the TDD loop)
    };
    let opts = RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: None,
        cache: None,
        semantic_cache: None,
        worktree: Some(&provider),
        progress: None,
        check: Some(&check),
    };
    let report = run(&wf, &json!("add a helper"), &mut backend, &opts);

    // The build step's change-set merged back into the working tree — a real
    // diff, not an analysis summary.
    assert!(
        !provider.applied.lock().unwrap().is_empty(),
        "implement phase must merge a change-set back into the working tree"
    );
    // The verify step actually ran the check command and stopped once green.
    assert!(
        rounds.get() >= 2,
        "verify must run the check command (the implement→test→repeat loop)"
    );
    let verify = report
        .phases
        .iter()
        .find(|phase| phase.id == "verify")
        .expect("verify phase present");
    assert_eq!(verify.rounds, 2, "verify greened on round 2");
}

#[test]
fn command_green_runs_to_max_rounds_when_never_green() {
    let wf = workflow(&json!({
        "name": "tdd",
        "phases": [{
            "id": "impl", "prompt": "fix {seen}",
            "repeat": { "max_rounds": 3, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));
    let mut backend = MockBackend::echo("r");
    let check = |_cmd: &str| -> i32 { 1 }; // never green
    let opts = RunOptions {
        check: Some(&check),
        ..fast_opts()
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(report.phases[0].rounds, 3, "exhausted max_rounds");
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("did not pass within 3 round(s)")),
        "notes: {:?}",
        report.notes
    );
}

#[test]
fn command_green_without_a_checker_runs_to_max_rounds() {
    // No check runner wired (opts.check = None): the engine never greens on
    // its own and never spawns a shell — it just exhausts max_rounds.
    let wf = workflow(&json!({
        "name": "tdd",
        "phases": [{
            "id": "impl", "prompt": "fix {seen}",
            "repeat": { "max_rounds": 2, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &Value::Null, &mut backend); // fast_opts → check: None
    assert_eq!(report.phases[0].rounds, 2);
    assert!(
        backend
            .spawns
            .iter()
            .all(|spawn| spawn.prior_failures == 0),
        "an unavailable checker is not evidence that implementation failed"
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("did not pass within 2 round(s)")),
        "notes: {:?}",
        report.notes
    );
}

#[test]
fn pipeline_threads_each_item_through_stages() {
    let wf = workflow(&json!({
        "name": "demo",
        "mode": "pipeline",
        "phases": [
            { "id": "s0", "fanout": ["x", "y"], "prompt": "stage0 {item}" },
            { "id": "s1", "prompt": "stage1 got {item}" }
        ]
    }));
    // stage0 transforms its item into R[item]; stage1 echoes the prompt it saw.
    let mut backend = MockBackend::new(|_, rec| {
        let task = delegated_task_prompt(&rec.prompt);
        if let Some(rest) = task.strip_prefix("stage0 ") {
            Outcome::Completed(format!("R[{rest}]"))
        } else {
            Outcome::Completed(task.to_string())
        }
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.status, "completed");
    assert_eq!(report.agents_spawned, 4, "2 chains × 2 stages");
    assert_eq!(report.phases.len(), 2);
    // stage 0: one result per chain.
    assert_eq!(report.phases[0].items.len(), 2);
    assert_eq!(report.phases[0].items[0].result.as_deref(), Some("R[x]"));
    assert_eq!(report.phases[0].items[1].result.as_deref(), Some("R[y]"));
    // stage 1: each chain's stage-0 result was threaded in as `{item}`.
    assert_eq!(report.phases[1].items.len(), 2);
    assert_eq!(report.phases[1].items[0].input, "R[x]");
    assert_eq!(
        report.phases[1].items[0].result.as_deref(),
        Some("stage1 got R[x]")
    );
    assert_eq!(report.phases[1].items[1].input, "R[y]");
    // chain identity (original item index) preserved across stages.
    assert_eq!(report.phases[1].items[0].index, 0);
    assert_eq!(report.phases[1].items[1].index, 1);
}

#[test]
fn pipeline_isolates_chain_failure_and_preserves_index() {
    let wf = workflow(&json!({
        "name": "demo",
        "mode": "pipeline",
        "phases": [
            { "id": "s0", "fanout": ["x", "y", "z"], "prompt": "s0 {item}" },
            { "id": "s1", "prompt": "s1 {item}" }
        ]
    }));
    // Chain 1 (spawn index 1, item "y") fails at stage 0; the others complete.
    let mut backend = MockBackend::new(|index, rec| {
        if rec.prompt.starts_with("s0 ") {
            if index == 1 {
                Outcome::Failed("boom".to_string())
            } else {
                Outcome::Completed(format!("ok{index}"))
            }
        } else {
            Outcome::Completed(delegated_task_prompt(&rec.prompt).to_string())
        }
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(
        report.status, "completed",
        "one chain failing does not fail the run"
    );
    // stage 0 records all three chains; chain 1 failed.
    assert_eq!(report.phases[0].items.len(), 3);
    assert_eq!(report.phases[0].items[1].status, STATUS_FAILED);
    // stage 1 only ran the two survivors, keeping their original indices.
    assert_eq!(report.phases[1].items.len(), 2);
    assert_eq!(report.phases[1].items[0].index, 0);
    assert_eq!(report.phases[1].items[1].index, 2);
    assert!(report.notes.iter().any(|n| n.contains("1 agent(s) failed")));
}

#[test]
fn pipeline_ignores_later_stage_source() {
    // `over` on a later pipeline stage is ignored — the chain supplies the
    // `{item}`, so stage `b` is a 1:1 threading of `a`, not a re-fan-out.
    let wf = workflow(&json!({
        "name": "demo",
        "mode": "pipeline",
        "phases": [
            { "id": "a", "fanout": ["x", "y"], "prompt": "one {item}" },
            { "id": "b", "over": "a", "prompt": "two {item}" }
        ]
    }));
    let mut backend = MockBackend::new(|_, rec| {
        let task = delegated_task_prompt(&rec.prompt);
        if let Some(rest) = task.strip_prefix("one ") {
            Outcome::Completed(format!("R{rest}"))
        } else {
            Outcome::Completed(task.to_string())
        }
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.phases.len(), 2);
    assert_eq!(report.phases[1].items.len(), 2);
    assert_eq!(report.phases[1].items[0].input, "Rx");
    assert_eq!(report.phases[1].items[1].input, "Ry");
}

#[test]
fn pipeline_budget_clamps_chains_and_skips_later_stages() {
    let wf = workflow(&json!({
        "name": "demo",
        "mode": "pipeline",
        "phases": [
            { "id": "s0", "fanout": ["a", "b", "c", "d"], "prompt": "s0 {item}" },
            { "id": "s1", "prompt": "s1 {item}" }
        ],
        "budget": { "max_agents": 3 }
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.agents_spawned, 3, "stage 0 clamped to the budget");
    assert!(report.budget_exhausted);
    assert_eq!(report.status, "budget_exhausted");
    assert_eq!(report.phases.len(), 1, "no budget left to start stage s1");
    assert_eq!(report.phases[0].items.len(), 3);
    assert!(report.notes.iter().any(|n| n.contains("dropped")));
}

#[test]
fn pipeline_single_stage_is_a_fan_out() {
    let wf = workflow(&json!({
        "name": "demo",
        "mode": "pipeline",
        "phases": [{ "id": "only", "fanout": ["a", "b"], "prompt": "do {item}" }]
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.status, "completed");
    assert_eq!(report.phases.len(), 1);
    assert_eq!(report.phases[0].items.len(), 2);
    assert_eq!(report.agents_spawned, 2);
}

#[test]
fn pipeline_synthesize_runs_over_completed_items() {
    let wf = workflow(&json!({
        "name": "demo",
        "mode": "pipeline",
        "phases": [
            { "id": "s0", "fanout": ["a"], "prompt": "s0 {item}" },
            { "id": "s1", "prompt": "s1 {item}" }
        ],
        "synthesize": { "prompt": "combine:\n{all}" }
    }));
    let mut backend = MockBackend::new(|_, rec| {
        if rec.description.contains("synthesize") {
            Outcome::Completed(format!("SUMMARY<<{}>>", rec.prompt))
        } else {
            Outcome::Completed(delegated_task_prompt(&rec.prompt).to_string())
        }
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.agents_spawned, 3, "1 chain × 2 stages + synthesize");
    let synthesis = report.synthesis.expect("synthesis present");
    assert!(synthesis.starts_with("SUMMARY<<combine:"));
}

#[test]
fn pipeline_cancel_between_stages_stops_after_current() {
    let wf = workflow(&json!({
        "name": "demo",
        "mode": "pipeline",
        "phases": [
            { "id": "s0", "fanout": ["a"], "prompt": "s0 {item}" },
            { "id": "s1", "prompt": "s1 {item}" }
        ]
    }));
    // Trip the flag while stage `s0` spawns; stage `s1`'s boundary check halts.
    let flag = Arc::new(AtomicBool::new(false));
    let trip = flag.clone();
    let mut backend = MockBackend::new(move |_, _| {
        trip.store(true, Ordering::Relaxed);
        Outcome::Completed("done".to_string())
    });
    let opts = RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: Some(&flag),
        cache: None,
        semantic_cache: None,
        worktree: None,
        progress: None,
        check: None,
    };
    let report = run(&wf, &Value::Null, &mut backend, &opts);

    assert_eq!(report.status, "cancelled");
    assert_eq!(report.phases.len(), 1, "only stage s0 ran");
    assert_eq!(report.phases[0].id, "s0");
}

#[test]
fn repeat_fixed_runs_all_rounds_and_accumulates() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "hunt", "fanout": ["round"], "prompt": "find {seen}",
            "repeat": { "max_rounds": 3, "until": "fixed" }
        }]
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.agents_spawned, 3, "one agent per round × 3 rounds");
    assert_eq!(report.phases[0].rounds, 3);
    assert_eq!(
        report.phases[0].items.len(),
        3,
        "no dedup → every round kept"
    );
    assert!(
        report.notes.iter().all(|n| !n.contains("roadmap")),
        "repeat is implemented — no deferral note"
    );
}

#[test]
fn repeat_injects_prior_rounds_into_seen() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "hunt", "fanout": ["round"], "prompt": "find {seen}",
            "repeat": { "max_rounds": 2, "until": "fixed" }
        }]
    }));
    let mut backend = MockBackend::echo("r");
    run_wf(&wf, &Value::Null, &mut backend);

    // Round 0 sees nothing; round 1 sees round 0's result ("r0").
    assert_eq!(delegated_task_prompt(&backend.spawns[0].prompt), "find ");
    assert_eq!(delegated_task_prompt(&backend.spawns[1].prompt), "find r0");
}

#[test]
fn repeat_no_new_terminates_when_round_is_all_duplicates() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "hunt", "fanout": ["round"], "prompt": "find",
            "schema": { "type": "object" },
            "repeat": { "max_rounds": 5, "until": "no_new", "dedup_by": "id" }
        }]
    }));
    // Every round returns the same dedup key → round 2 adds nothing new.
    let mut backend = MockBackend::new(|_, _| Outcome::Completed("{\"id\":\"X\"}".to_string()));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(
        report.phases[0].rounds, 2,
        "stop the round after the first with no new keys"
    );
    assert_eq!(report.agents_spawned, 2);
    assert_eq!(report.phases[0].items.len(), 1, "deduped union keeps one");
}

#[test]
fn repeat_no_new_runs_full_when_every_round_is_new() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "hunt", "fanout": ["round"], "prompt": "find",
            "schema": { "type": "object" },
            "repeat": { "max_rounds": 3, "until": "no_new", "dedup_by": "id" }
        }]
    }));
    // Distinct key per call → never "no new" → runs the full cap.
    let mut backend =
        MockBackend::new(|index, _| Outcome::Completed(format!("{{\"id\":\"k{index}\"}}")));
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.phases[0].rounds, 3);
    assert_eq!(report.phases[0].items.len(), 3, "all distinct → all kept");
}

#[test]
fn dedup_path_extracts_keys() {
    assert_eq!(
        extract_dedup_keys(
            &json!({ "bugs": [{ "title": "a" }, { "title": "b" }] }),
            "bugs[].title"
        ),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        extract_dedup_keys(&json!({ "id": 7 }), "id"),
        vec!["7".to_string()]
    );
    assert_eq!(
        extract_dedup_keys(&json!({ "a": { "b": "deep" } }), "a.b"),
        vec!["deep".to_string()]
    );
    assert!(extract_dedup_keys(&json!({ "x": 1 }), "missing").is_empty());
}

// --- resume cache (step 7) ---------------------------------------------

/// In-memory cache: replays a fixed prefix from `load`, records every
/// `store` for assertions. `&self` store needs interior mutability.
struct MockCache {
    preload: Option<Vec<PhaseReport>>,
    stored: std::cell::RefCell<Vec<Vec<PhaseReport>>>,
}

impl MockCache {
    fn empty() -> Self {
        Self {
            preload: None,
            stored: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn with_prefix(prefix: Vec<PhaseReport>) -> Self {
        Self {
            preload: Some(prefix),
            stored: std::cell::RefCell::new(Vec::new()),
        }
    }
}

impl WorkflowCache for MockCache {
    fn load(&self) -> Option<Vec<PhaseReport>> {
        self.preload.clone()
    }

    fn store(&self, phases: &[PhaseReport]) {
        self.stored.borrow_mut().push(phases.to_vec());
    }
}

/// A fabricated completed phase report, as if a prior run had cached it.
fn cached_phase(id: &str, result: &str) -> PhaseReport {
    PhaseReport {
        id: id.to_string(),
        rounds: 1,
        output_tokens: 0,
        relay_bytes: 0,
        carried_pass_count: 0,
        retried_finding_count: 0,
        skipped_count: 0,
        blocked_finding_count: 0,
        escalated_finding_count: 0,
        findings: Vec::new(),
        pass_receipts: Vec::new(),
        items: vec![ItemResult {
            index: 0,
            input: "cached-input".to_string(),
            agent_id: "cached".to_string(),
            status: STATUS_COMPLETED.to_string(),
            result: Some(result.to_string()),
            error: None,
            structured: None,
            output_tokens: 0,
            loaded_skills: Vec::new(),
            semantic_verdict: None,
            retry_key: None,
            carry_reason: None,
            carried: false,
        }],
    }
}

fn opts_with_cache(cache: &dyn WorkflowCache) -> RunOptions<'_> {
    RunOptions {
        phase_timeout: Duration::from_millis(50),
        cancel: None,
        cache: Some(cache),
        semantic_cache: None,
        worktree: None,
        progress: None,
        check: None,
    }
}

fn two_phase_wf() -> NormalizedWorkflow {
    workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "p0", "fanout": ["x"], "prompt": "do {item}" },
            { "id": "p1", "over": "p0", "prompt": "verify {item}" }
        ]
    }))
}

#[test]
fn resume_replays_cached_phase_without_spawning() {
    let wf = two_phase_wf();
    let cache = MockCache::with_prefix(vec![cached_phase("p0", "cached-result")]);
    // Echo the prompt so we can prove the cached phase fed the next one.
    let mut backend = MockBackend::new(|_, rec| {
        Outcome::Completed(delegated_task_prompt(&rec.prompt).to_string())
    });
    let report = run(&wf, &Value::Null, &mut backend, &opts_with_cache(&cache));

    assert_eq!(report.status, "completed");
    // Phase p0 came from cache (no spawn); only p1 ran.
    assert_eq!(report.agents_spawned, 1, "only the uncached phase spawned");
    assert_eq!(
        report.phases[0].items[0].result.as_deref(),
        Some("cached-result")
    );
    // p1 mapped `over` p0's *cached* completed result.
    assert_eq!(report.phases[1].items.len(), 1);
    assert_eq!(
        report.phases[1].items[0].result.as_deref(),
        Some("verify cached-result")
    );
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("resumed 1 phase(s)")));
}

#[test]
fn resume_command_green_revalidates_current_tree() {
    // A command_green (verify) phase cached green by a prior run. The run id keys
    // only (spec, input), so a resume must re-run the check against the *current*
    // tree before trusting that green (BUG-R5).
    let wf = workflow(&json!({
        "name": "verify-only",
        "phases": [{
            "id": "verify",
            "prompt": "run checks",
            "repeat": { "max_rounds": 2, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));

    // Tree now RED → the cached green is stale → re-check, then re-run the phase.
    let red_cache = MockCache::with_prefix(vec![cached_phase("verify", "stale-green")]);
    let mut backend = MockBackend::echo("r");
    let red_calls = std::cell::Cell::new(0u32);
    let red_check = |_cmd: &str| -> i32 {
        red_calls.set(red_calls.get() + 1);
        1 // never green
    };
    let red_opts = RunOptions {
        cache: Some(&red_cache),
        check: Some(&red_check),
        ..fast_opts()
    };
    let red = run(&wf, &Value::Null, &mut backend, &red_opts);
    assert!(
        red_calls.get() >= 1,
        "the cached green must be re-checked on resume"
    );
    assert!(
        red.agents_spawned >= 1,
        "a stale-green phase must re-run, not replay"
    );
    assert!(
        red.notes
            .iter()
            .any(|note| note.contains("no longer") && note.contains("re-running")),
        "notes: {:?}",
        red.notes
    );

    // Contrast: still green → replay from cache, no spawn.
    let green_cache = MockCache::with_prefix(vec![cached_phase("verify", "fresh-green")]);
    let mut backend2 = MockBackend::echo("r");
    let green_check = |_cmd: &str| -> i32 { 0 };
    let green_opts = RunOptions {
        cache: Some(&green_cache),
        check: Some(&green_check),
        ..fast_opts()
    };
    let green = run(&wf, &Value::Null, &mut backend2, &green_opts);
    assert_eq!(
        green.agents_spawned, 0,
        "a still-green phase replays from cache without spawning"
    );
    assert!(green
        .notes
        .iter()
        .any(|note| note.contains("resumed 1 phase(s)")));
}

#[test]
fn resume_full_cache_spawns_nothing() {
    let wf = two_phase_wf();
    let cache = MockCache::with_prefix(vec![cached_phase("p0", "r0"), cached_phase("p1", "r1")]);
    let mut backend = MockBackend::echo("should-not-run");
    let report = run(&wf, &Value::Null, &mut backend, &opts_with_cache(&cache));

    assert_eq!(
        report.agents_spawned, 0,
        "a full cache replays the whole run"
    );
    assert_eq!(report.status, "completed");
    assert_eq!(report.phases.len(), 2);
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("resumed 2 phase(s)")));
}

#[test]
fn cache_miss_runs_and_stores_each_phase_prefix() {
    let wf = two_phase_wf();
    let cache = MockCache::empty();
    let mut backend = MockBackend::echo("r");
    let report = run(&wf, &Value::Null, &mut backend, &opts_with_cache(&cache));

    assert_eq!(report.agents_spawned, 2, "nothing cached → both phases ran");
    assert!(report.notes.iter().all(|n| !n.contains("resumed")));
    // store() flushed the growing prefix after each phase: [p0], then [p0,p1].
    let stored = cache.stored.borrow();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].len(), 1);
    assert_eq!(stored[1].len(), 2);
    assert_eq!(stored[1][1].id, "p1");
}

#[test]
fn cache_with_mismatched_id_reruns_phase() {
    // A prefix whose position-0 id does not match the spec's first phase is
    // rejected (the id guard), so the phase runs fresh rather than replaying
    // a stale/colliding entry.
    let wf = two_phase_wf();
    let cache = MockCache::with_prefix(vec![cached_phase("WRONG", "stale")]);
    let mut backend = MockBackend::echo("r");
    let report = run(&wf, &Value::Null, &mut backend, &opts_with_cache(&cache));

    assert_eq!(
        report.agents_spawned, 2,
        "mismatched cache is ignored — both phases ran"
    );
    assert_ne!(report.phases[0].items[0].result.as_deref(), Some("stale"));
    assert!(report.notes.iter().all(|n| !n.contains("resumed")));
}

/// The live `implement`-failed-but-`verify`-still-ran bug: a phase whose
/// EVERY agent fails halts the run — later phases must not spawn against
/// work that never happened, and the run must not read `completed`.
#[test]
fn all_failed_phase_halts_later_phases_and_fails_the_run() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "implement", "prompt": "build it" },
            { "id": "verify", "prompt": "verify the artifacts" }
        ]
    }));
    let mut backend = MockBackend::new(|_, _| {
        Outcome::Failed("http error: error decoding response body".to_string())
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.phases.len(), 1, "verify must not run");
    assert_eq!(report.phases[0].items[0].status, STATUS_FAILED);
    assert_eq!(report.status, "failed", "an undelivered run must not read green");
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("produced no completed item")),
        "the halt must be explained: {:?}",
        report.notes
    );
    assert_eq!(
        backend.spawns.len(),
        1,
        "no verify agent may spawn after implement fails"
    );
}

/// Partial failure keeps flowing: one failed sibling does not halt the
/// pipeline of phases (guards against over-tightening the halt gate).
#[test]
fn partially_failed_phase_still_runs_later_phases() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [
            { "id": "implement", "fanout": ["a", "b"], "prompt": "do {item}" },
            { "id": "verify", "prompt": "verify the artifacts" }
        ]
    }));
    let mut backend = MockBackend::new(|index, _| match index {
        0 => Outcome::Failed("boom".to_string()),
        _ => Outcome::Completed("ok".to_string()),
    });
    let report = run_wf(&wf, &Value::Null, &mut backend);

    assert_eq!(report.phases.len(), 2, "verify still runs");
    assert_eq!(report.status, "completed");
}

// --- {seen} relay bounds ------------------------------------------------

fn completed_item(index: usize, result: &str) -> ItemResult {
    ItemResult {
        index,
        input: format!("item-{index}"),
        agent_id: format!("agent-{index}"),
        status: STATUS_COMPLETED.to_string(),
        result: Some(result.to_string()),
        error: None,
        structured: None,
        output_tokens: 0,
        loaded_skills: Vec::new(),
        semantic_verdict: None,
        retry_key: None,
        carry_reason: None,
        carried: false,
    }
}

/// `{seen}` is re-rendered into EVERY item's prompt on EVERY round, so an
/// uncapped result line is paid for roughly `items x rounds` times — the one
/// agent-result relay in the engine that had no cap at all, while the
/// synthesis path capped the same data at 2 KiB per item.
#[test]
fn the_seen_relay_caps_each_line_because_every_item_pays_for_it_every_round() {
    let huge = "finding: ".to_string() + &"detail ".repeat(4_000);
    let text = seen_text(&[completed_item(0, &huge)]);

    assert!(
        text.len() < huge.len() / 4,
        "an uncapped line is the most expensive byte in the engine: {} bytes",
        text.len()
    );
    assert!(
        text.starts_with("finding: detail"),
        "the identifying head must survive — recognition is the relay's job: {text}"
    );
}

/// A four-figure fan-out assembles a prompt-dominating block even out of
/// individually-modest lines, so the total is capped too — and the omission is
/// STATED. A silently-partial avoidance list reads as "these were never found",
/// which sends the next round to re-find them: exactly what the relay prevents.
#[test]
fn the_seen_relay_states_what_it_omitted_rather_than_dropping_it_silently() {
    let items: Vec<ItemResult> = (0..2_000)
        .map(|index| completed_item(index, &format!("finding {index}: {}", "x".repeat(300))))
        .collect();
    let text = seen_text(&items);

    assert!(text.len() < 30_000, "total stayed bounded: {} bytes", text.len());
    assert!(
        text.starts_with('[') && text.contains("omitted for length"),
        "the omission must be visible to the agent reading it: {}",
        &text[..120.min(text.len())]
    );
    assert!(
        text.contains("finding 1999"),
        "the newest results are the ones kept"
    );
}

/// A run small enough to fit is relayed verbatim — the caps only trim the
/// pathological case, and they never announce an omission that did not happen.
#[test]
fn a_small_seen_relay_is_untouched_and_claims_no_omission() {
    let text = seen_text(&[completed_item(0, "found A"), completed_item(1, "found B")]);
    assert_eq!(text, "found A\nfound B");
    assert!(seen_text(&[]).is_empty(), "nothing seen yet is empty, not a notice");
}

// --- relay auto-injection ------------------------------------------------

/// A repeat phase whose author never named `{seen}` still gets the relay, and
/// gets it with the instruction attached.
///
/// Measured on real runs before this existed: 112 of 112 repeat phases used
/// `command_green`, and 46 of the 65 that executed carried no relay. Those
/// spent 40% of their output tokens after round 1 against 26% for
/// relay-bearing phases, and re-reported an already-reported file 3.0 times per
/// round-pair against 0.33 — one of them restated an already-green verify
/// command for 5,923 tokens.
#[test]
fn a_repeat_prompt_that_never_named_seen_still_receives_the_relay() {
    let rendered = super::prompts::render_prompt(
        "fix the failing test",
        "item",
        0,
        "task",
        "round 1 found: the retry path drops the error",
        None,
    );
    assert!(rendered.starts_with("fix the failing test"), "author's prompt is untouched");
    assert!(rendered.contains("round 1 found: the retry path drops the error"));
    // The instruction must ride along: the phases this was measured against were
    // preset-authored and carried their own "do not churn" sentence, so relaying
    // the evidence without the behavior would not transfer the effect.
    assert!(
        rendered.contains("Do not re-derive"),
        "the relay carries the instruction, not just the data: {rendered}"
    );
}

/// It costs nothing where there is nothing to relay. Round 1 and every
/// non-repeat phase render exactly as before — which is also what keeps
/// `verifier_fingerprint` (which renders with `seen = ""`) byte-stable.
#[test]
fn an_empty_relay_leaves_the_prompt_byte_identical() {
    let plain = super::prompts::render_prompt("do the work", "item", 0, "task", "", None);
    assert_eq!(plain, "do the work");
}

/// A phase that DID name `{seen}` gets exactly one copy — the substitution, in
/// the author's chosen position — never a second appended block.
#[test]
fn an_authored_seen_token_is_not_duplicated_by_the_injection() {
    let rendered = super::prompts::render_prompt(
        "prior:\n{seen}\nnow continue",
        "item",
        0,
        "task",
        "ROUND1",
        None,
    );
    assert_eq!(rendered.matches("ROUND1").count(), 1);
    assert!(!rendered.contains("Do not re-derive"));
}

/// The relay is appended BEFORE the schema instruction, so the JSON-only
/// directive stays the last thing the agent reads.
#[test]
fn the_schema_instruction_stays_last_when_the_relay_is_injected() {
    let schema = serde_json::json!({"type": "object"});
    let rendered =
        super::prompts::render_prompt("work", "item", 0, "task", "PRIOR", Some(&schema));
    let relay_at = rendered.find("PRIOR").expect("relay present");
    let schema_at = rendered.find("StructuredOutput").expect("schema instruction present");
    assert!(relay_at < schema_at, "schema instruction must remain last:\n{rendered}");
}

/// The relay must be invisible to the semantic-pass cache key. That key is
/// what lets a receipt stored in round 1 still be found in round 2, so if the
/// injection reached it every stored receipt would be orphaned — silently, and
/// nothing else in the suite would notice.
#[test]
fn the_relay_never_moves_the_verifier_fingerprint() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "impl", "prompt": "implement it",
            "repeat": { "max_rounds": 3, "until": { "command_green": { "command": "cargo test" } } }
        }]
    }));
    let phase = &wf.phases[0];
    let fingerprint = super::spawn::verifier_fingerprint(phase, "task", "item");
    assert!(
        !fingerprint.contains("Do not re-derive"),
        "the cache key must not carry the relay: {fingerprint}"
    );
    // Stable across calls for the same phase — the property rounds depend on.
    assert_eq!(
        fingerprint,
        super::spawn::verifier_fingerprint(phase, "task", "item")
    );
}

/// The durable receipt. Reconstructing relay presence from spec text is what
/// produced two disagreeing answers (82% vs 71%) before this field existed,
/// because a preset expands its relay engine-side and never appears in the
/// authored spec. `state/workflows/<run>.json` outlives the session, so the
/// count belongs there.
#[test]
fn a_repeat_phase_records_the_bytes_it_relayed() {
    let wf = workflow(&json!({
        "name": "demo",
        "phases": [{
            "id": "hunt", "fanout": ["round"], "prompt": "find",
            "repeat": { "max_rounds": 2, "until": "fixed" }
        }]
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&wf, &Value::Null, &mut backend);
    let phase = report.phases.first().expect("one phase");
    assert_eq!(phase.rounds, 2);
    assert!(
        phase.relay_bytes > 0,
        "round 2 carried round 1's results, so the receipt must be non-zero"
    );

    // A single-round phase has nothing earlier to carry, and says so with zero
    // rather than by omitting the axis.
    let once = workflow(&json!({
        "name": "demo",
        "phases": [{ "id": "solo", "prompt": "do it" }]
    }));
    let mut backend = MockBackend::echo("r");
    let report = run_wf(&once, &Value::Null, &mut backend);
    assert_eq!(report.phases[0].relay_bytes, 0);
}

fn skilled_item(index: usize, result: &str, skills: &[&str]) -> ItemResult {
    ItemResult {
        loaded_skills: skills.iter().map(|s| (*s).to_string()).collect(),
        ..completed_item(index, result)
    }
}

/// The receipt is a fixed 210+ byte sentence. Charged per line against a
/// 512-byte cap it took 44% of the budget of a block whose entire job is that
/// each line be RECOGNIZABLE — and the same sentence, repeated once per line,
/// was the largest single consumer of the 24 KB total. It says one thing about
/// the whole round, so it is stated once for the whole block.
#[test]
fn the_seen_relay_spends_its_line_budget_on_results_not_on_a_repeated_receipt() {
    let body = "finding: ".to_string() + &"detail ".repeat(4_000);
    let items: Vec<ItemResult> = (0..4)
        .map(|index| skilled_item(index, &body, &["frontend-design"]))
        .collect();
    let text = seen_text(&items);

    assert_eq!(
        text.matches("Workflow skill receipt").count(),
        1,
        "one receipt for the block, not one per line: {}",
        &text[..200.min(text.len())]
    );
    assert!(
        text.contains("frontend-design"),
        "the receipt must still name what was loaded"
    );
    for line in text.lines().filter(|line| line.starts_with("finding: ")) {
        assert!(
            line.len() > 500,
            "a capped line is nearly all result, not preamble: {} bytes",
            line.len()
        );
    }
    assert_eq!(
        text.lines().filter(|line| line.starts_with("finding: ")).count(),
        4,
        "every item still relays its own result"
    );
}

/// Skills are deduped across the accumulator in first-seen order, and an
/// accumulator that loaded nothing pays nothing.
#[test]
fn the_seen_receipt_names_each_loaded_skill_once_and_is_absent_when_none_loaded() {
    let text = seen_text(&[
        skilled_item(0, "found A", &["frontend-design"]),
        skilled_item(1, "found B", &["frontend-design", "dataviz"]),
    ]);
    let receipt = text.lines().next().expect("a receipt line");
    assert_eq!(receipt.matches("frontend-design").count(), 1);
    assert!(receipt.contains("frontend-design, dataviz"));
    assert!(text.ends_with("found A\nfound B"), "results follow it: {text}");

    let plain = seen_text(&[completed_item(0, "found A")]);
    assert_eq!(plain, "found A", "no skills loaded, no preamble");
}

/// The emptiness filter used to measure the banner-prefixed string, so a
/// completed item that produced no result survived as a line of pure
/// boilerplate — 225 bytes carrying nothing to recognize.
#[test]
fn a_resultless_skilled_item_relays_no_line_at_all() {
    let mut item = skilled_item(0, "", &["frontend-design"]);
    item.result = None;
    assert!(
        seen_text(&[item]).is_empty(),
        "an item with nothing to relay contributes nothing, receipt included"
    );
}
