use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use core_types::helper_run::HelperRun;
use serde_json::{json, Value};

use super::{Checkpoint, GraphRunOptions};
use crate::misc_tools::{AgentCompletion, AgentInput};
use crate::workflow_tools::engine::{self, AgentBackend, RunOptions};
use crate::workflow_tools::graph_store::GraphStore;
use crate::workflow_tools::spec::{NormalizedWorkflow, WorkflowSpec};
use crate::ToolError;

#[derive(Default)]
struct MemoryStore {
    checkpoint: RefCell<Option<Value>>,
    saves: Cell<usize>,
    fail_save: Cell<Option<usize>>,
    cancel_at: Option<(u32, Arc<AtomicBool>)>,
}

impl MemoryStore {
    fn cancelling_after(step: u32, cancel: &Arc<AtomicBool>) -> Self {
        Self {
            cancel_at: Some((step, Arc::clone(cancel))),
            ..Self::default()
        }
    }

    fn snapshot(&self) -> Value {
        self.checkpoint.borrow().clone().expect("a persisted checkpoint")
    }
}

impl GraphStore for MemoryStore {
    fn load(&self) -> Result<Option<Value>, String> {
        Ok(self.checkpoint.borrow().clone())
    }

    fn save(&self, checkpoint: &Value) -> Result<(), String> {
        self.saves.set(self.saves.get() + 1);
        if self.fail_save.get() == Some(self.saves.get()) {
            return Err("injected checkpoint write failure".to_string());
        }
        *self.checkpoint.borrow_mut() = Some(checkpoint.clone());
        if let Some((step, cancel)) = &self.cancel_at {
            if checkpoint["steps"] == *step && checkpoint["in_flight"] == false {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Reply {
    status: &'static str,
    structured: Option<Value>,
    error: Option<String>,
    tokens: u64,
    live: bool,
}

impl Default for Reply {
    fn default() -> Self {
        Self {
            status: "completed",
            structured: None,
            error: None,
            tokens: 0,
            live: false,
        }
    }
}

#[derive(Default)]
struct Backend {
    replies: VecDeque<Reply>,
    fallback: Reply,
    prompts: Vec<String>,
    completions: BTreeMap<String, AgentCompletion>,
    live: BTreeSet<String>,
}

impl AgentBackend for Backend {
    fn spawn(&mut self, input: AgentInput) -> Result<String, ToolError> {
        let id = format!("graph-agent-{}", self.prompts.len());
        self.prompts.push(input.prompt);
        let reply = self.replies.pop_front().unwrap_or_else(|| self.fallback.clone());
        if reply.live {
            self.live.insert(id.clone());
        }
        self.completions.insert(id.clone(), AgentCompletion {
            agent_id: id.clone(),
            name: id.clone(),
            status: reply.status.to_string(),
            result: Some("completed fixture work".to_string()),
            structured: reply.structured,
            error: reply.error,
            run: HelperRun { output_tokens: reply.tokens, ..HelperRun::default() },
        });
        Ok(id)
    }

    fn wait(&self, ids: &[String], _timeout: Duration) -> Vec<AgentCompletion> {
        ids.iter().filter_map(|id| self.completions.get(id).cloned()).collect()
    }

    fn worker_is_live(&self, id: &str) -> bool {
        self.live.contains(id)
    }

    fn concurrency_window(&self) -> usize {
        1
    }
}

#[allow(clippy::needless_pass_by_value)] // JSON fixture helper accepts inline json! values.
fn workflow(value: Value) -> NormalizedWorkflow {
    WorkflowSpec::from_value(&value).expect("graph JSON").validate().expect("valid graph")
}

fn options(store: &MemoryStore) -> RunOptions<'_> {
    let mut options = RunOptions::production();
    options.graph = Some(GraphRunOptions {
        strict_accounting: false,
        check: None,
        store,
        fingerprint: "same-spec-and-input",
        session_id: "original-session",
        dream: None,
    });
    options
}

fn repair_graph() -> Value {
    json!({
        "name": "repair", "mode": "graph",
        "phases": [{"id": "fix", "prompt": "Repair this failure: {item}"}],
        "graph": {"entry": "check", "max_steps": 8, "nodes": [
            {"kind": "check", "id": "check", "command": "cargo test", "on_pass": "dream", "on_fail": "fix"},
            {"kind": "agent", "id": "fix", "phase": "fix", "next": "check", "on_error": "failed"},
            {"kind": "dream", "id": "dream", "next": "done"},
            {"kind": "end", "id": "done", "status": "completed"},
            {"kind": "end", "id": "failed", "status": "failed"}
        ]}
    })
}

fn two_agents() -> Value {
    json!({
        "name": "two-agents", "mode": "graph",
        "phases": [{"id": "work", "prompt": "Continue from {item}"}],
        "graph": {"entry": "first", "max_steps": 3, "nodes": [
            {"kind": "agent", "id": "first", "phase": "work", "next": "second"},
            {"kind": "agent", "id": "second", "phase": "work", "next": "done"},
            {"kind": "end", "id": "done", "status": "completed"}
        ]}
    })
}

#[test]
fn red_check_repairs_reverifies_and_dreams_only_the_fresh_green() {
    let workflow = workflow(repair_graph());
    let store = MemoryStore::default();
    let mut backend = Backend::default();
    let checks = Cell::new(0);
    let check = |command: &str| {
        assert_eq!(command, "cargo test");
        checks.set(checks.get() + 1);
        i32::from(checks.get() == 1)
    };
    let dreams = RefCell::new(Vec::new());
    let dream = |commands: &[String], session: &str| {
        dreams.borrow_mut().push((commands.to_vec(), session.to_string()));
        Ok(json!({"promoted": 0, "observations": 1}))
    };
    let mut opts = options(&store);
    opts.check = Some(&check);
    opts.graph.as_mut().unwrap().dream = Some(&dream);

    let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);

    assert_eq!(report.status, "completed");
    assert_eq!(checks.get(), 2);
    assert_eq!(backend.prompts.len(), 1);
    assert!(backend.prompts[0].contains("\"exit_code\":1"), "the failed check reaches repair");
    assert_eq!(*dreams.borrow(), [(vec!["cargo test".to_string()], "original-session".to_string())]);
    assert_eq!(report.graph.as_ref().unwrap()["steps"], 5);
    assert_eq!(store.snapshot()["terminal"], "completed");
}

#[test]
fn a_graph_check_relays_its_diagnostic_receipt_into_the_repair_prompt() {
    let workflow = workflow(repair_graph());
    let store = MemoryStore::default();
    let mut backend = Backend::default();
    let calls = Cell::new(0);
    let check = |command: &str| {
        assert_eq!(command, "cargo test");
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            super::CheckResult {
                exit_code: 1,
                stdout: "test summary: 1 failed".to_string(),
                stderr: "expected 2, found 3".to_string(),
            }
        } else {
            super::CheckResult {
                exit_code: 0,
                stdout: "all tests passed".to_string(),
                stderr: String::new(),
            }
        }
    };
    let legacy = |_: &str| -> i32 { panic!("graph receipt checker takes precedence") };
    let dream = |_: &[String], _: &str| Ok(json!({"promoted": 0}));
    let mut opts = options(&store);
    opts.check = Some(&legacy);
    let graph = opts.graph.as_mut().unwrap();
    graph.check = Some(&check);
    graph.dream = Some(&dream);

    let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);

    assert_eq!(report.status, "completed");
    assert_eq!(calls.get(), 2);
    assert_eq!(backend.prompts.len(), 1);
    assert!(backend.prompts[0].contains("test summary: 1 failed"));
    assert!(backend.prompts[0].contains("expected 2, found 3"));
    assert!(report.phases[0].items[0].input.contains("expected 2, found 3"));
    assert_eq!(report.graph.as_ref().unwrap()["outputs"]["check"]["stdout"], "all tests passed");
}

#[test]
fn graph_worktree_creation_failure_never_falls_back_to_a_shared_spawn() {
    use crate::workflow_tools::worktree::{WorktreeGuard, WorktreeProvider};

    struct UnavailableWorktree(Cell<usize>);

    impl WorktreeProvider for UnavailableWorktree {
        fn create(&self, _label: &str) -> Result<Box<dyn WorktreeGuard>, String> {
            self.0.set(self.0.get() + 1);
            Err("injected worktree creation failure".to_string())
        }
    }

    let mut value = two_agents();
    value["isolation"] = json!("worktree");
    value["apply"] = json!("sequential");
    let workflow = workflow(value);
    let store = MemoryStore::default();
    let provider = UnavailableWorktree(Cell::new(0));
    let mut backend = Backend::default();
    let opts = options(&store).with_worktree(&provider);

    let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);

    assert_eq!(report.status, "blocked");
    assert_eq!(provider.0.get(), 1);
    assert!(backend.prompts.is_empty(), "failed isolation must not launch against the shared tree");
    assert_eq!(report.agents_spawned, 0);
}

#[test]
fn structured_branch_runs_only_its_selected_successor() {
    let workflow = workflow(json!({
        "name": "route", "mode": "graph",
        "phases": [
            {"id": "route", "prompt": "Choose a route", "schema": {"type":"object", "properties":{"route":{"type":"string"}}, "required":["route"]}},
            {"id": "left", "prompt": "LEFT {item}"},
            {"id": "right", "prompt": "RIGHT {item}"}
        ],
        "graph": {"entry":"route", "max_steps":4, "nodes":[
            {"kind":"agent", "id":"route", "phase":"route", "next":"choose"},
            {"kind":"branch", "id":"choose", "source":"route", "pointer":"/items/0/structured/route", "equals":"left", "on_match":"left", "on_miss":"right"},
            {"kind":"agent", "id":"left", "phase":"left", "next":"done"},
            {"kind":"agent", "id":"right", "phase":"right", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"}
        ]}
    }));
    for route in ["left", "right"] {
        let store = MemoryStore::default();
        let mut backend = Backend::default();
        backend.replies.push_back(Reply { structured: Some(json!({"route":route})), ..Reply::default() });
        let report = engine::run(&workflow, &Value::Null, &mut backend, &options(&store));
        assert_eq!(report.status, "completed");
        assert_eq!(backend.prompts.len(), 2);
        assert!(backend.prompts[1].starts_with(&route.to_uppercase()), "{:?}", backend.prompts);
        assert_eq!(report.graph.as_ref().unwrap()["visits"][2]["node"], route);
    }
}

#[test]
fn cycle_visits_share_one_step_budget() {
    let mut value = repair_graph();
    value["graph"]["max_steps"] = json!(4);
    let workflow = workflow(value);
    let store = MemoryStore::default();
    let mut backend = Backend::default();
    let checks = Cell::new(0);
    let check = |_: &str| { checks.set(checks.get() + 1); 1 };
    let mut opts = options(&store);
    opts.check = Some(&check);

    let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);

    assert_eq!(report.status, "budget_exhausted");
    assert!(report.budget_exhausted);
    assert_eq!(report.graph.as_ref().unwrap()["steps"], 4);
    assert_eq!(checks.get(), 2);
    assert_eq!(backend.prompts.len(), 2);
    assert_eq!(store.snapshot()["cursor"], "check");
}

#[test]
fn cancellation_at_a_boundary_resumes_without_repeating_the_completed_agent() {
    let workflow = workflow(two_agents());
    let cancel = Arc::new(AtomicBool::new(false));
    let store = MemoryStore::cancelling_after(1, &cancel);
    let mut first = Backend::default();
    let mut opts = options(&store);
    opts.cancel = Some(&cancel);

    assert_eq!(engine::run(&workflow, &Value::Null, &mut first, &opts).status, "cancelled");
    assert_eq!(first.prompts.len(), 1);
    assert_eq!(store.snapshot()["cursor"], "second");
    assert_eq!(store.snapshot()["in_flight"], false);

    cancel.store(false, Ordering::Relaxed);
    opts.graph.as_mut().unwrap().session_id = "resuming-session";
    let mut resumed = Backend::default();
    let report = engine::run(&workflow, &Value::Null, &mut resumed, &opts);
    assert_eq!(report.status, "completed");
    assert_eq!(resumed.prompts.len(), 1);
    assert!(resumed.prompts[0].contains("completed fixture work"));
    assert_eq!(report.agents_spawned, 2);
    assert_eq!(store.snapshot()["session_id"], "original-session");
}

#[test]
fn agent_and_token_budgets_survive_a_cancelled_run() {
    for budget in [json!({"max_agents":1}), json!({"max_output_tokens":5})] {
        let mut value = two_agents();
        value["budget"] = budget;
        let workflow = workflow(value);
        let cancel = Arc::new(AtomicBool::new(false));
        let store = MemoryStore::cancelling_after(1, &cancel);
        let mut first = Backend { fallback: Reply { tokens:7, ..Reply::default() }, ..Backend::default() };
        let mut opts = options(&store);
        opts.cancel = Some(&cancel);
        assert_eq!(engine::run(&workflow, &Value::Null, &mut first, &opts).status, "cancelled");

        cancel.store(false, Ordering::Relaxed);
        let mut resumed = Backend::default();
        let report = engine::run(&workflow, &Value::Null, &mut resumed, &opts);
        assert_eq!(report.status, "budget_exhausted");
        assert_eq!(report.agents_spawned, 1);
        assert_eq!(report.output_tokens, 7);
        assert!(resumed.prompts.is_empty(), "a resume cannot buy a new budget");
    }
}

#[test]
fn uncertain_intent_and_changed_fingerprint_never_execute() {
    let workflow = workflow(repair_graph());
    for changed_fingerprint in [false, true] {
        let store = MemoryStore::default();
        let mut opts = options(&store);
        let mut checkpoint = Checkpoint::new(&workflow, opts.graph.as_ref().unwrap());
        if changed_fingerprint {
            checkpoint.fingerprint = "a-different-spec".to_string();
        } else {
            checkpoint.in_flight = true;
        }
        store.save(&serde_json::to_value(checkpoint).unwrap()).unwrap();
        let check = |_: &str| -> i32 { panic!("uncertain work must not be checked again") };
        let dream = |_: &[String], _: &str| -> Result<Value, String> { panic!("no new learning from an invalid resume") };
        opts.check = Some(&check);
        opts.graph.as_mut().unwrap().dream = Some(&dream);
        let mut backend = Backend::default();
        let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);
        assert_eq!(report.status, "blocked");
        assert!(backend.prompts.is_empty());
    }
}

#[test]
fn checkpoint_failures_stop_before_more_effects_and_preserve_uncertain_intent() {
    let workflow = workflow(repair_graph());
    for failing_save in [1, 2] {
        let store = MemoryStore::default();
        store.fail_save.set(Some(failing_save));
        let mut backend = Backend::default();
        let checked = Cell::new(0);
        let check = |_: &str| { checked.set(checked.get() + 1); 0 };
        let mut opts = options(&store);
        opts.check = Some(&check);

        let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);
        assert_eq!(report.status, "blocked");
        assert_eq!(checked.get(), failing_save - 1);
        assert!(backend.prompts.is_empty());
        if failing_save == 1 {
            assert!(store.checkpoint.borrow().is_none());
        } else {
            assert_eq!(store.snapshot()["in_flight"], true);
            assert_eq!(store.snapshot()["cursor"], "check");
            let shown = report.graph.as_ref().expect("the durable intent is reported");
            assert_eq!(shown["in_flight"], true);
            assert_eq!(shown["cursor"], "check");
            assert_eq!(shown["steps"], 0);
            store.fail_save.set(None);
            let resumed = engine::run(&workflow, &Value::Null, &mut backend, &opts);
            assert_eq!(resumed.status, "blocked");
            assert_eq!(checked.get(), 1, "lost result must not replay its effect");
        }
    }
}

#[test]
fn unavailable_check_is_not_a_quality_failure_that_enters_repair() {
    let workflow = workflow(repair_graph());
    let store = MemoryStore::default();
    let mut backend = Backend::default();
    let check = |_: &str| -1;
    let mut opts = options(&store);
    opts.check = Some(&check);
    let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);
    assert_eq!(report.status, "blocked");
    assert!(backend.prompts.is_empty());
    assert_eq!(store.snapshot()["in_flight"], true);
    assert_eq!(store.snapshot()["cursor"], "check");
}

#[test]
fn stopped_or_still_live_workers_never_retry_or_take_an_error_edge() {
    let workflow = workflow(json!({
        "name":"uncertain-agent", "mode":"graph",
        "phases":[{"id":"work", "prompt":"Work"}],
        "graph":{"entry":"work", "max_steps":3, "nodes":[
            {"kind":"agent", "id":"work", "phase":"work", "next":"done", "on_error":"recover"},
            {"kind":"agent", "id":"recover", "phase":"work", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"}
        ]}
    }));
    for reply in [
        Reply { status:"stopped", error:Some(engine::PHASE_TIMEOUT_STOP_ERROR.to_string()), ..Reply::default() },
        Reply { status:"still_running", ..Reply::default() },
        Reply { live:true, ..Reply::default() },
    ] {
        let store = MemoryStore::default();
        let mut backend = Backend { fallback:reply, ..Backend::default() };
        let report = engine::run(&workflow, &Value::Null, &mut backend, &options(&store));
        assert_eq!(report.status, "blocked");
        assert_eq!(backend.prompts.len(), 1, "only the explicit graph may repeat work");
        assert_eq!(store.snapshot()["in_flight"], true);
    }
}

#[test]
fn dreaming_refuses_unverified_or_invalidated_checks() {
    let graphs = [
        json!({"entry":"dream", "max_steps":2, "nodes":[
            {"kind":"dream", "id":"dream", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"}
        ]}),
        json!({"entry":"check", "max_steps":4, "nodes":[
            {"kind":"check", "id":"check", "command":"cargo test", "on_pass":"edit", "on_fail":"failed"},
            {"kind":"agent", "id":"edit", "phase":"work", "next":"dream"},
            {"kind":"dream", "id":"dream", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"},
            {"kind":"end", "id":"failed", "status":"failed"}
        ]}),
        json!({"entry":"red", "max_steps":4, "nodes":[
            {"kind":"check", "id":"red", "command":"red", "on_pass":"green", "on_fail":"green"},
            {"kind":"check", "id":"green", "command":"green", "on_pass":"dream", "on_fail":"failed"},
            {"kind":"dream", "id":"dream", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"},
            {"kind":"end", "id":"failed", "status":"failed"}
        ]}),
    ];
    for graph in graphs {
        let workflow = workflow(json!({
            "name":"learning-gate", "mode":"graph", "graph":graph,
            "phases":[{"id":"work", "prompt":"Mutate workspace"}]
        }));
        let store = MemoryStore::default();
        let mut backend = Backend::default();
        let check = |command: &str| i32::from(command == "red");
        let dream = |_: &[String], _: &str| -> Result<Value, String> { panic!("no verified current evidence") };
        let mut opts = options(&store);
        opts.check = Some(&check);
        opts.graph.as_mut().unwrap().dream = Some(&dream);
        let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);
        assert_eq!(report.status, "blocked");
        assert!(report.notes.iter().any(|note| note.contains("dream requires")));
    }
}

#[test]
fn terminal_replay_does_not_repeat_verification_or_dreaming() {
    let workflow = workflow(repair_graph());
    let store = MemoryStore::default();
    let mut backend = Backend::default();
    let checked = Cell::new(0);
    let dreamed = Cell::new(0);
    let check = |_: &str| { checked.set(checked.get() + 1); 0 };
    let dream = |_: &[String], _: &str| { dreamed.set(dreamed.get() + 1); Ok(json!({"promoted":0})) };
    let mut opts = options(&store);
    opts.check = Some(&check);
    opts.graph.as_mut().unwrap().dream = Some(&dream);
    let first = engine::run(&workflow, &Value::Null, &mut backend, &opts);
    let replayed = engine::run(&workflow, &Value::Null, &mut backend, &opts);
    assert_eq!(first.status, "completed");
    assert_eq!(replayed.status, "completed");
    assert_eq!(replayed.graph.as_ref().unwrap()["resumed"], true);
    assert_eq!(replayed.graph.as_ref().unwrap()["verified"], true);
    assert_eq!(replayed.graph.as_ref().unwrap()["verification_scope"], "recorded_run");
    assert_eq!(checked.get(), 1);
    assert_eq!(dreamed.get(), 1);
    assert!(backend.prompts.is_empty());
}

#[test]
fn an_empty_input_fanout_is_not_successful_agent_work() {
    let workflow = workflow(json!({
        "name":"empty", "mode":"graph",
        "phases":[{"id":"work", "prompt":"Work on {item}", "fanout":["$input"]}],
        "graph":{"entry":"work", "max_steps":2, "nodes":[
            {"kind":"agent", "id":"work", "phase":"work", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"}
        ]}
    }));
    let store = MemoryStore::default();
    let mut backend = Backend::default();
    let report = engine::run(&workflow, &json!([]), &mut backend, &options(&store));
    assert_eq!(report.status, "failed");
    assert!(backend.prompts.is_empty());
    assert_eq!(store.snapshot()["visits"][0]["outcome"], "failed");
}

#[test]
fn missing_branch_data_blocks_instead_of_taking_the_miss_edge() {
    let workflow = workflow(json!({
        "name":"missing-data", "mode":"graph",
        "phases":[{"id":"work", "prompt":"Work"}],
        "graph":{"entry":"branch", "max_steps":3, "nodes":[
            {"kind":"branch", "id":"branch", "source":"work", "pointer":"/items/0/structured/answer", "equals":true, "on_match":"done", "on_miss":"work"},
            {"kind":"agent", "id":"work", "phase":"work", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"}
        ]}
    }));
    let store = MemoryStore::default();
    let mut backend = Backend::default();
    let report = engine::run(&workflow, &Value::Null, &mut backend, &options(&store));
    assert_eq!(report.status, "blocked");
    assert!(backend.prompts.is_empty());
    assert!(report.notes.iter().any(|note| note.contains("has no value")));
}

#[test]
fn a_nonterminal_resume_cannot_dream_from_an_earlier_process_green() {
    let workflow = workflow(repair_graph());
    let cancel = Arc::new(AtomicBool::new(false));
    let store = MemoryStore::cancelling_after(1, &cancel);
    let mut backend = Backend::default();
    let checked = Cell::new(0);
    let dreamed = Cell::new(0);
    let check = |_: &str| {
        checked.set(checked.get() + 1);
        assert_eq!(checked.get(), 1, "a resumed graph must not repeat a completed check");
        0
    };
    let dream = |_: &[String], _: &str| {
        dreamed.set(dreamed.get() + 1);
        Ok(json!({"promoted": 1}))
    };
    let mut opts = options(&store);
    opts.cancel = Some(&cancel);
    opts.check = Some(&check);
    opts.graph.as_mut().unwrap().dream = Some(&dream);
    let first = engine::run(&workflow, &Value::Null, &mut backend, &opts);
    assert_eq!(first.status, "cancelled");
    assert_eq!(store.snapshot()["cursor"], "dream");
    assert_eq!(store.snapshot()["checks"]["cargo test"], true);

    cancel.store(false, Ordering::Relaxed);
    let resumed = engine::run(&workflow, &Value::Null, &mut backend, &opts);
    assert_eq!(resumed.status, "blocked");
    assert_eq!(resumed.graph.as_ref().unwrap()["verified"], false);
    assert_eq!(dreamed.get(), 0, "stale evidence cannot write a learned lesson");
    assert_eq!(checked.get(), 1);
    assert!(backend.prompts.is_empty());
}

#[test]
fn syntactically_valid_but_inconsistent_checkpoints_refuse_all_work() {
    let workflow = workflow(repair_graph());
    let cancel = Arc::new(AtomicBool::new(false));
    let initial = MemoryStore::cancelling_after(1, &cancel);
    let check = |_: &str| 0;
    let mut first_options = options(&initial);
    first_options.cancel = Some(&cancel);
    first_options.check = Some(&check);
    let first = engine::run(&workflow, &Value::Null, &mut Backend::default(), &first_options);
    assert_eq!(first.status, "cancelled");
    let valid = initial.snapshot();

    for (pointer, replacement) in [
        ("/terminal", json!("completed")),
        ("/terminal", json!("invented-success")),
        ("/cursor", json!("missing-node")),
        ("/usage/cost", json!(-0.1)),
        ("/visits/0/node", json!("dream")),
        ("/visits/0/next", json!("failed")),
        ("/visits/0/step", json!(2)),
        ("/steps", json!(2)),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).expect("checkpoint field") = replacement;
        let store = MemoryStore {
            checkpoint: RefCell::new(Some(invalid)),
            ..MemoryStore::default()
        };
        let mut backend = Backend::default();
        let no_check = |_: &str| -> i32 { panic!("invalid checkpoint ran a command") };
        let no_dream = |_: &[String], _: &str| -> Result<Value, String> {
            panic!("invalid checkpoint wrote memory")
        };
        let mut opts = options(&store);
        opts.check = Some(&no_check);
        opts.graph.as_mut().unwrap().dream = Some(&no_dream);
        let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);
        assert_eq!(report.status, "blocked", "accepted mutation of {pointer}");
        assert!(backend.prompts.is_empty(), "accepted mutation of {pointer}");
        assert_eq!(store.saves.get(), 0, "invalid state was overwritten at {pointer}");
    }
}

#[test]
fn a_budget_truncated_fanout_cannot_follow_its_success_edge() {
    let workflow = workflow(json!({
        "name":"partial-fanout", "mode":"graph", "budget":{"max_agents":1},
        "phases":[{"id":"work", "prompt":"Process {item}", "fanout":["first", "second"]}],
        "graph":{"entry":"work", "max_steps":2, "nodes":[
            {"kind":"agent", "id":"work", "phase":"work", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"}
        ]}
    }));
    let store = MemoryStore::default();
    let mut backend = Backend::default();
    let opts = options(&store);
    let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);
    assert_eq!(report.status, "budget_exhausted");
    assert!(report.budget_exhausted);
    assert_eq!(backend.prompts.len(), 1);
    assert_eq!(report.graph.as_ref().unwrap()["steps"], 1);
    assert_eq!(store.snapshot()["visits"][0]["outcome"], "budget_exhausted");
    assert_eq!(store.snapshot()["terminal"], "budget_exhausted");

    let mut resumed = Backend::default();
    let replayed = engine::run(&workflow, &Value::Null, &mut resumed, &opts);
    assert_eq!(replayed.status, "budget_exhausted");
    assert!(resumed.prompts.is_empty());
}

#[test]
fn scheduled_graph_stops_when_a_worker_has_no_measured_usage() {
    let workflow = workflow(json!({
        "name":"unmetered", "mode":"graph",
        "phases":[{"id":"work", "prompt":"Do bounded work"}],
        "graph":{"entry":"work", "max_steps":3, "nodes":[
            {"kind":"agent", "id":"work", "phase":"work", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"}
        ]}
    }));
    let store = MemoryStore::default();
    let mut opts = options(&store);
    opts.graph.as_mut().unwrap().strict_accounting = true;
    let mut backend = Backend::default();
    let report = engine::run(&workflow, &Value::Null, &mut backend, &opts);
    assert_eq!(report.status, "blocked");
    assert_eq!(backend.prompts.len(), 1);
    assert_eq!(report.graph.unwrap()["accounting_complete"], false);
}
