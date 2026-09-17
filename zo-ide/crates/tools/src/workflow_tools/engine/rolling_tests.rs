use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use core_types::helper_run::HelperRun;
use serde_json::json;

use super::super::spec::WorkflowSpec;
use super::super::wait::WaitState;
use super::spawn::SpawnUnit;
use super::*;
use crate::misc_tools::AgentUsageCost;

struct VirtualJob {
    start: u64,
    done: u64,
    exit: u64,
    rejected: bool,
}

struct VirtualBackend<'a> {
    durations: &'a [u64],
    jobs: Vec<VirtualJob>,
    now: Cell<u64>,
    width: usize,
    peak: usize,
    linger_first: u64,
    first_stop_marker: Option<&'static str>,
    reject: Option<usize>,
    readonly: bool,
    barrier_calls: Cell<usize>,
    next_calls: Cell<usize>,
    cancelled: RefCell<Vec<String>>,
    cancel_at: Option<(&'a AtomicBool, u64)>,
    expire_on_observation: bool,
    receipt: AgentUsageCost,
}

impl<'a> VirtualBackend<'a> {
    fn new(durations: &'a [u64], width: usize) -> Self {
        Self {
            durations, jobs: Vec::new(), now: Cell::new(0), width, peak: 0,
            linger_first: 0, first_stop_marker: None, reject: None, readonly: true,
            barrier_calls: Cell::new(0), next_calls: Cell::new(0),
            cancelled: RefCell::new(Vec::new()), cancel_at: None,
            expire_on_observation: false,
            receipt: AgentUsageCost { requests: 1, estimated_cost_usd: 0.5, ..Default::default() },
        }
    }

    fn job(&self, id: &str) -> &VirtualJob {
        &self.jobs[id.strip_prefix("job-").unwrap().parse::<usize>().unwrap()]
    }

    fn completions(&self, ids: &[String]) -> Vec<AgentCompletion> {
        ids.iter().map(|id| {
            let done = self.now.get() >= self.job(id).done;
            let mut completion = AgentCompletion {
                agent_id: id.clone(), name: id.clone(),
                status: if done { STATUS_COMPLETED } else { STATUS_STILL_RUNNING }.to_string(),
                result: done.then(|| id.clone()), structured: None, error: None,
                run: HelperRun { output_tokens: u64::from(done), ..Default::default() },
            };
            if let Some(marker) = self.first_stop_marker.filter(|_| done && id == "job-0") {
                completion.status = STATUS_STOPPED.to_string();
                if self.now.get() == self.job(id).done {
                    completion.error = Some(marker.to_string());
                    completion.result = Some("salvaged partial".to_string());
                } else {
                    completion.error = Some("runtime cancelled".to_string());
                    completion.result = None;
                    completion.run.output_tokens = 2;
                }
            }
            completion
        }).collect()
    }
}

impl AgentBackend for VirtualBackend<'_> {
    fn spawn(&mut self, _input: AgentInput) -> Result<String, ToolError> {
        let live = self.jobs.iter().filter(|job| !job.rejected && job.exit > self.now.get()).count();
        assert!(live < self.width, "a physical worker still owns the slot");
        let index = self.jobs.len();
        let done = self.now.get() + self.durations[index];
        let rejected = self.reject == Some(index);
        self.jobs.push(VirtualJob {
            start: self.now.get(), done,
            exit: done + if index == 0 { self.linger_first } else { 0 }, rejected,
        });
        if rejected {
            return Err(ToolError::Execution("fixture spawn rejection".to_string()));
        }
        self.peak = self.peak.max(live + 1);
        Ok(format!("job-{index}"))
    }

    fn allows_readonly_refill(&self) -> bool { self.readonly }

    fn wait(&self, ids: &[String], _timeout: Duration) -> Vec<AgentCompletion> {
        self.barrier_calls.set(self.barrier_calls.get() + 1);
        if let Some(end) = ids.iter().map(|id| self.job(id).done).max() {
            self.now.set(self.now.get().max(end));
        }
        self.completions(ids)
    }

    fn wait_next(&self, ids: &[String], _timeout: Duration, watch: &mut WaitState) -> Vec<AgentCompletion> {
        if self.next_calls.get() > 0 {
            assert!(watch.startup_extensions.contains("fixture"), "watch state was reset");
        }
        watch.startup_extensions.insert("fixture".to_string());
        self.next_calls.set(self.next_calls.get() + 1);
        let next = ids.iter().flat_map(|id| [self.job(id).done, self.job(id).exit])
            .filter(|end| *end > self.now.get()).min();
        if let Some(next) = next { self.now.set(next); }
        if let Some((flag, at)) = self.cancel_at {
            if self.now.get() >= at { flag.store(true, Ordering::Relaxed); }
        }
        if self.expire_on_observation {
            *watch = WaitState::new(Instant::now(), Duration::ZERO);
        }
        self.completions(ids)
    }

    fn worker_is_live(&self, id: &str) -> bool {
        !id.is_empty() && self.now.get() < self.job(id).exit
    }

    fn cancel(&self, id: &str) -> Option<AgentCompletion> {
        self.cancelled.borrow_mut().push(id.to_string());
        Some(AgentCompletion {
            agent_id: id.to_string(), name: id.to_string(), status: STATUS_STOPPED.to_string(),
            result: None, structured: None, error: Some("fixture cancellation".to_string()),
            run: HelperRun::default(),
        })
    }

    fn usage_cost(&self, _id: &str) -> Option<AgentUsageCost> { Some(self.receipt.clone()) }
}

fn phase_and_units(count: usize) -> (NormalizedPhase, Vec<SpawnUnit>) {
    let workflow = WorkflowSpec::from_value(&json!({
        "name": "offline", "phases": [{ "id": "inspect", "prompt": "inspect {item}" }]
    })).and_then(WorkflowSpec::validate).unwrap();
    let phase = workflow.phases.into_iter().next().unwrap();
    let units = (0..count).map(|index| SpawnUnit {
        index, item: index.to_string(), prompt: format!("inspect {index}"), prior_failures: 0,
    }).collect();
    (phase, units)
}

fn opts() -> RunOptions<'static> {
    RunOptions {
        phase_timeout: Duration::from_secs(60), cancel: None, cache: None,
        semantic_cache: None, worktree: None, progress: None, check: None,
    }
}

fn run_virtual(durations: &[u64], width: usize, refill: bool) -> (u64, Vec<ItemResult>) {
    let (phase, units) = phase_and_units(durations.len());
    let mut backend = VirtualBackend::new(durations, width);
    let mut state = EngineState::new(None, None, None, 0.0);
    let items = if refill {
        rolling::spawn_and_collect(&phase, units, &mut backend, &opts(), &mut state, width)
    } else {
        spawn::spawn_batched(&phase, units, &mut backend, &opts(), &mut state, None, width)
    };
    assert_eq!(backend.jobs.len(), durations.len(), "benchmark skipped work");
    assert_eq!(items.len(), durations.len(), "benchmark lost a result");
    assert!(backend.peak <= width);
    assert_eq!(state.agents_spawned, durations.len());
    assert_eq!(state.output_tokens_spent, durations.len() as u64);
    for (index, item) in items.iter().enumerate() {
        assert_eq!(item.index, index, "result order changed");
        assert_eq!(item.status, STATUS_COMPLETED);
        assert_eq!(item.result.as_deref(), Some(format!("job-{index}").as_str()));
    }
    (backend.now.get(), items)
}

#[test]
fn readonly_refill_reuses_fast_slot_before_slow_sibling_finishes() {
    let durations = [2, 20, 8, 1];
    let (phase, units) = phase_and_units(durations.len());
    let mut backend = VirtualBackend::new(&durations, 2);
    let mut state = EngineState::new(None, None, None, 0.0);
    let items = rolling::spawn_and_collect(&phase, units, &mut backend, &opts(), &mut state, 2);
    assert_eq!(backend.jobs.iter().map(|job| job.start).collect::<Vec<_>>(), [0, 0, 2, 10]);
    assert_eq!(items.iter().map(|item| item.index).collect::<Vec<_>>(), [0, 1, 2, 3]);
    assert_eq!(state.usage_cost.unwrap().requests, 4);
    assert_eq!(backend.barrier_calls.get(), 0);
}

#[test]
fn readonly_refill_holds_logical_completion_until_physical_exit() {
    let (phase, units) = phase_and_units(3);
    let mut backend = VirtualBackend::new(&[1, 20, 1], 2);
    backend.linger_first = 5;
    let mut state = EngineState::new(None, None, None, 0.0);
    let items = rolling::spawn_and_collect(&phase, units, &mut backend, &opts(), &mut state, 2);
    assert_eq!(backend.jobs[2].start, 6);
    assert_eq!(items.len(), 3);
    assert_eq!(state.usage_cost.unwrap().requests, 3, "reobservations must not double bill");
    assert!(backend.cancelled.borrow().is_empty());
}

#[test]
fn readonly_refill_preserves_watchdog_salvage_and_final_usage() {
    for marker in [PHASE_TIMEOUT_STOP_ERROR, STARTUP_NO_PROGRESS_STOP_ERROR] {
        let (phase, units) = phase_and_units(2);
        let mut backend = VirtualBackend::new(&[1, 20], 2);
        backend.linger_first = 5;
        backend.first_stop_marker = Some(marker);
        let mut state = EngineState::new(None, None, None, 0.0);
        let items = rolling::spawn_and_collect(&phase, units, &mut backend, &opts(), &mut state, 2);
        assert_eq!(items[0].status, STATUS_STOPPED);
        assert_eq!(items[0].error.as_deref(), Some(marker));
        assert_eq!(items[0].result.as_deref(), Some("salvaged partial"));
        assert_eq!(state.output_tokens_spent, 3);
        assert_eq!(state.usage_cost.unwrap().requests, 2);
        assert!(backend.cancelled.borrow().is_empty());
    }
}

#[test]
fn readonly_refill_preserves_completed_result_during_cancellation() {
    let cancel = AtomicBool::new(false);
    let (phase, units) = phase_and_units(2);
    let mut backend = VirtualBackend::new(&[1, 10], 2);
    backend.linger_first = 5;
    backend.cancel_at = Some((&cancel, 1));
    let mut state = EngineState::new(None, None, None, 0.0);
    let opts = RunOptions { cancel: Some(&cancel), ..opts() };
    let items = rolling::spawn_and_collect(&phase, units, &mut backend, &opts, &mut state, 2);
    assert_eq!(items[0].status, STATUS_COMPLETED);
    assert_eq!(items[0].result.as_deref(), Some("job-0"));
    assert!(items[0].error.is_none());
    assert_eq!(items[1].status, STATUS_STOPPED);
    assert!(state.spawn_block.is_some());
    assert!(backend.worker_is_live("job-0"));
}

#[test]
fn readonly_refill_checks_agent_token_cost_and_incomplete_budgets() {
    for (agents, tokens, cost, incomplete) in [
        (Some(2), None, None, false),
        (None, Some(1), None, false),
        (None, None, Some(0.5), false),
        (None, None, Some(10.0), true),
    ] {
        let (phase, units) = phase_and_units(4);
        let mut backend = VirtualBackend::new(&[1, 10, 1, 1], 2);
        backend.receipt.unpriced_requests = u64::from(incomplete);
        let mut state = EngineState::new(agents, tokens, cost, 0.0);
        let items = rolling::spawn_and_collect(&phase, units, &mut backend, &opts(), &mut state, 2);
        assert_eq!(backend.jobs.len(), 2);
        assert_eq!(items.len(), 2, "unspawned units are not failed results");
        assert!(items.iter().all(|item| item.status == STATUS_COMPLETED));
        assert!(state.budget_exhausted);
    }
}

#[test]
fn readonly_refill_defers_budgeted_spawns_while_terminal_usage_can_grow() {
    let (phase, units) = phase_and_units(3);
    let mut backend = VirtualBackend::new(&[1, 5, 1], 2);
    backend.linger_first = 9;
    let mut state = EngineState::new(None, None, Some(1.0), 0.0);
    let items = rolling::spawn_and_collect(&phase, units, &mut backend, &opts(), &mut state, 2);
    assert_eq!(backend.jobs.len(), 2, "unsettled terminal cost allowed a refill");
    assert_eq!(items.len(), 2);
    assert!((state.cost_usd_spent - 1.0).abs() < f64::EPSILON);
}

#[test]
fn readonly_refill_cancel_and_deadline_stop_spawning_and_cancel_survivors() {
    for expire in [false, true] {
        let cancel = AtomicBool::new(false);
        let (phase, units) = phase_and_units(4);
        let mut backend = VirtualBackend::new(&[1, 10, 1, 1], 2);
        backend.expire_on_observation = expire;
        if !expire { backend.cancel_at = Some((&cancel, 1)); }
        let mut state = EngineState::new(None, None, None, 0.0);
        let opts = RunOptions { cancel: Some(&cancel), ..opts() };
        let items = rolling::spawn_and_collect(&phase, units, &mut backend, &opts, &mut state, 2);
        assert_eq!(backend.jobs.len(), 2);
        assert_eq!(items.len(), 2);
        assert!(backend.cancelled.borrow().iter().any(|id| id == "job-1"));
        assert!(backend.worker_is_live("job-1"), "logical stop is not physical exit");
        assert!(state.spawn_block.is_some());
        assert!(!state.budget_exhausted, "a wait stop is not a budget failure");
        let (next_phase, next_units) = phase_and_units(1);
        let next = spawn::spawn_and_collect(
            &next_phase, "", next_units, &mut backend, &opts, &mut state, None,
        );
        assert!(next.is_empty());
        assert_eq!(backend.jobs.len(), 2);
    }
}

#[test]
fn readonly_refill_stop_cannot_report_success_or_start_a_later_phase() {
    let workflow = WorkflowSpec::from_value(&json!({
        "name": "stop", "phases": [
            {"id": "first", "fanout": ["a", "b"], "prompt": "inspect"},
            {"id": "later", "prompt": "must not spawn"}
        ]
    })).and_then(WorkflowSpec::validate).unwrap();
    let mut backend = VirtualBackend::new(&[1, 10, 1], 2);
    backend.expire_on_observation = true;
    let report = run(&workflow, &Value::Null, &mut backend, &opts());
    assert_eq!(report.status, "failed");
    assert!(!report.budget_exhausted);
    assert_eq!(report.phases.len(), 1);
    assert_eq!(report.agents_spawned, 2);
    assert!(report.notes.iter().any(|note| note.contains("further spawns blocked")));
}

#[test]
fn readonly_refill_handles_spawn_error_and_unknown_cost_without_budget() {
    let (phase, units) = phase_and_units(4);
    let mut backend = VirtualBackend::new(&[1, 10, 1, 1], 2);
    backend.reject = Some(1);
    backend.receipt.unreported_requests = 1;
    let mut state = EngineState::new(None, None, None, 0.0);
    let items = rolling::spawn_and_collect(&phase, units, &mut backend, &opts(), &mut state, 2);
    assert_eq!(items.len(), 4);
    assert_eq!(items[1].status, STATUS_FAILED);
    assert_eq!(state.agents_spawned, 3);
    assert_eq!(state.usage_cost.unwrap().unreported_requests, 3);
}

#[test]
fn readonly_refill_production_dispatch_requires_backend_opt_in() {
    let width = crate::misc_tools::workflow_concurrency_limit().max(1);
    let durations = vec![1; width + 1];
    for readonly in [false, true] {
        let (phase, units) = phase_and_units(durations.len());
        let mut backend = VirtualBackend::new(&durations, width);
        backend.readonly = readonly;
        let mut state = EngineState::new(None, None, None, 0.0);
        let items = spawn::spawn_and_collect(&phase, "", units, &mut backend, &opts(), &mut state, None);
        assert_eq!(items.len(), durations.len());
        assert_eq!(backend.next_calls.get() > 0, readonly);
        assert_eq!(backend.barrier_calls.get() > 0, !readonly);
    }
}

#[test]
fn readonly_refill_isolation_keeps_the_batch_path_even_when_creation_fails() {
    use super::super::worktree::{WorktreeGuard, WorktreeProvider};
    struct Unavailable;
    impl WorktreeProvider for Unavailable {
        fn create(&self, _label: &str) -> Result<Box<dyn WorktreeGuard>, String> {
            Err("fixture unavailable".to_string())
        }
    }
    let width = crate::misc_tools::workflow_concurrency_limit().max(1);
    let durations = vec![1; width + 1];
    let (phase, units) = phase_and_units(durations.len());
    let mut backend = VirtualBackend::new(&durations, width);
    let mut state = EngineState::new(None, None, None, 0.0);
    let iso = spawn::IsolationCtx { provider: &Unavailable, merge_back: true };
    let items = spawn::spawn_and_collect(&phase, "", units, &mut backend, &opts(), &mut state, Some(iso));
    assert_eq!(items.len(), durations.len());
    assert_eq!(backend.next_calls.get(), 0);
    assert_eq!(backend.barrier_calls.get(), 2);
    assert_eq!(state.worktree_fallbacks, durations.len());
}

#[test]
fn readonly_refill_late_spawn_gets_its_own_inactivity_window() {
    let snapshot = AgentActivitySnapshot { started_at: Some(1000), ..Default::default() };
    assert!(!phase_inactivity_exceeded(&snapshot, 100, 1001, Duration::from_secs(30)));
    assert!(phase_inactivity_exceeded(&snapshot, 100, 1030, Duration::from_secs(30)));
}

#[test]
fn readonly_refill_offline_benchmark() {
    let mut faster = 0;
    let mut equal = 0;
    for len in 0..=6_u32 {
        for mut code in 0..3_usize.pow(len) {
            let durations: Vec<u64> = (0..len).map(|_| {
                let duration = [1, 2, 5][code % 3];
                code /= 3;
                duration
            }).collect();
            for width in 1..=3 {
                let (batch, _) = run_virtual(&durations, width, false);
                let (refill, _) = run_virtual(&durations, width, true);
                assert!(refill <= batch, "{durations:?}, width {width}: {refill} > {batch}");
                if refill < batch { faster += 1; } else { equal += 1; }
            }
        }
    }
    assert_eq!(faster + equal, 3279);
    assert!(faster > 0, "the purported refill path did no useful replenishment");
    let batch = run_virtual(&[2, 20, 8, 1], 2, false).0;
    let refill = run_virtual(&[2, 20, 8, 1], 2, true).0;
    assert_eq!((batch, refill), (28, 20));
    println!("offline virtual-time executor benchmark: 3279 cases, faster={faster}, equal={equal}, slower=0; example batch={batch} ticks, refill={refill} ticks; no LLM/API or real-time performance measurement");
}
