//! Bounded graph visits over the existing phase executor. A durable intent is
//! written before work; an interrupted visit is never replayed automatically.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{AgentBackend, EngineState, ItemResult, PhaseReport, RunOptions, WorkflowReport};
use crate::workflow_tools::graph_store::GraphStore;
use crate::workflow_tools::spec::{GraphEndStatus, GraphNode, NormalizedWorkflow, PhaseSource};

pub(crate) type DreamCallback<'a> = dyn Fn(&[String], &str) -> Result<Value, String> + 'a;

#[derive(Default)]
pub(crate) struct CheckResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) struct GraphRunOptions<'a> {
    pub strict_accounting: bool,
    pub check: Option<&'a dyn Fn(&str) -> CheckResult>,
    pub store: &'a dyn GraphStore,
    /// SHA-256 of the authored spec and input. Resume never accepts an edit.
    pub fingerprint: &'a str,
    pub session_id: &'a str,
    pub dream: Option<&'a DreamCallback<'a>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Checkpoint {
    version: u32,
    fingerprint: String,
    session_id: String,
    cursor: String,
    steps: u32,
    in_flight: bool,
    terminal: Option<String>,
    previous: Value,
    outputs: BTreeMap<String, Value>,
    prior: HashMap<String, Vec<ItemResult>>,
    phases: Vec<PhaseReport>,
    visits: Vec<Visit>,
    /// Only checks performed since the most recent agent visit are current.
    checks: BTreeMap<String, bool>,
    usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Visit {
    step: u32,
    node: String,
    outcome: String,
    next: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Usage {
    agents: usize,
    tokens: u64,
    cost: f64,
    cost_receipt: Option<crate::misc_tools::AgentUsageCost>,
    unmetered: usize,
}

impl Usage {
    fn capture(state: &EngineState) -> Self {
        Self {
            agents: state.agents_spawned,
            tokens: state.output_tokens_spent,
            cost: state.cost_usd_spent,
            cost_receipt: state.usage_cost.clone(),
            unmetered: state.unmetered_agents,
        }
    }

    fn restore(&self, state: &mut EngineState) {
        state.agents_spawned = self.agents;
        state.output_tokens_spent = self.tokens;
        state.cost_usd_spent = self.cost;
        state.usage_cost.clone_from(&self.cost_receipt);
        state.unmetered_agents = self.unmetered;
    }
}

impl Checkpoint {
    fn new(workflow: &NormalizedWorkflow, options: &GraphRunOptions<'_>) -> Self {
        Self {
            version: 1,
            fingerprint: options.fingerprint.to_string(),
            session_id: options.session_id.to_string(),
            cursor: workflow.graph.as_ref().expect("validated graph").entry.clone(),
            steps: 0,
            in_flight: false,
            terminal: None,
            previous: Value::Null,
            outputs: BTreeMap::new(),
            prior: HashMap::new(),
            phases: Vec::new(),
            visits: Vec::new(),
            checks: BTreeMap::new(),
            usage: Usage::default(),
        }
    }

    fn save(&self, store: &dyn GraphStore) -> Result<(), String> {
        let value = serde_json::to_value(self).map_err(|e| e.to_string())?;
        store.save(&value)
    }

    fn verified(&self) -> bool {
        !self.checks.is_empty() && self.checks.values().all(|passed| *passed)
    }
}

fn validate_checkpoint(checkpoint: &Checkpoint, workflow: &NormalizedWorkflow) -> Result<(), String> {
    let invalid = || "graph checkpoint has inconsistent execution state".to_string();
    let graph = workflow.graph.as_ref().expect("validated graph");
    if checkpoint.steps as usize != checkpoint.visits.len()
        || checkpoint.steps > graph.max_steps
        || !checkpoint.usage.cost.is_finite() || checkpoint.usage.cost < 0.0 {
        return Err(invalid());
    }
    let mut expected = graph.entry.as_str();
    let mut terminal = None;
    let mut agent_nodes = Vec::new();
    for (index, visit) in checkpoint.visits.iter().enumerate() {
        if terminal.is_some() || visit.node != expected || visit.step as usize != index + 1 {
            return Err(invalid());
        }
        let node = graph.nodes.iter().find(|node| node.id() == visit.node).ok_or_else(invalid)?;
        let (next, end) = match (node, visit.outcome.as_str()) {
            (GraphNode::Agent { next, .. }, "completed") | (GraphNode::Dream { next, .. }, "dreamed") => (Some(next.as_str()), None),
            (GraphNode::Agent { .. }, "budget_exhausted") => (None, Some("budget_exhausted")),
            (GraphNode::Agent { on_error, .. }, "failed") => (on_error.as_deref(), on_error.is_none().then_some("failed")),
            (GraphNode::Check { on_pass, .. }, "passed") => (Some(on_pass.as_str()), None),
            (GraphNode::Check { on_fail, .. }, "failed") => (Some(on_fail.as_str()), None),
            (GraphNode::Branch { on_match, .. }, "matched") => (Some(on_match.as_str()), None),
            (GraphNode::Branch { on_miss, .. }, "missed") => (Some(on_miss.as_str()), None),
            (GraphNode::End { status: GraphEndStatus::Completed, .. }, "completed") => (None, Some("completed")),
            (GraphNode::End { status: GraphEndStatus::Failed, .. }, "failed") => (None, Some("failed")),
            _ => return Err(invalid()),
        };
        if next != visit.next.as_deref() { return Err(invalid()); }
        if matches!(node, GraphNode::Agent { .. }) { agent_nodes.push(node.id()); }
        expected = next.unwrap_or_else(|| node.id());
        terminal = end;
    }
    let tokens = checkpoint.phases.iter().fold(0_u64, |sum, phase| sum.saturating_add(phase.output_tokens));
    let agents: usize = checkpoint.phases.iter().flat_map(|phase| &phase.items).filter(|item| !item.agent_id.is_empty()).count();
    if checkpoint.cursor != expected || checkpoint.terminal.as_deref() != terminal
        || checkpoint.phases.iter().map(|phase| phase.id.as_str()).collect::<Vec<_>>() != agent_nodes
        || checkpoint.usage.tokens < tokens || checkpoint.usage.agents < agents {
        return Err(invalid());
    }
    Ok(())
}

pub(super) const fn node_kind(node: &GraphNode) -> &'static str {
    match node {
        GraphNode::Agent { .. } => "agent", GraphNode::Check { .. } => "check",
        GraphNode::Branch { .. } => "branch", GraphNode::Dream { .. } => "dream",
        GraphNode::End { .. } => "end",
    }
}

struct Transition {
    output: Value,
    outcome: &'static str,
    next: Option<String>,
    terminal: Option<&'static str>,
}

#[expect(clippy::too_many_lines, reason = "one durable intent/result boundary surrounds every graph transition")]
pub(super) fn run(
    workflow: &NormalizedWorkflow,
    input: &Value,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions<'_>,
) -> WorkflowReport {
    let mut state = EngineState::new(
        workflow.max_agents, workflow.max_output_tokens, workflow.max_cost_usd,
        backend.output_price_per_token(),
    );
    let Some(options) = &opts.graph else {
        return report(workflow, None, &state, "blocked", false,
            vec!["graph requires a durable checkpoint store".to_string()]);
    };
    let loaded = options.store.load().and_then(|value| {
        value.map(|value| serde_json::from_value::<Checkpoint>(value)
            .map_err(|e| format!("invalid graph checkpoint: {e}"))).transpose()
    });
    let (mut checkpoint, resumed) = match loaded {
        Ok(Some(checkpoint)) => (checkpoint, true),
        Ok(None) => (Checkpoint::new(workflow, options), false),
        Err(error) => return report(workflow, None, &state, "blocked", false, vec![error]),
    };
    if checkpoint.version != 1 || checkpoint.fingerprint != options.fingerprint {
        return report(workflow, None, &state, "blocked", resumed,
            vec!["graph checkpoint does not match this spec and input; use a new runId for new work".to_string()]);
    }
    if let Err(error) = validate_checkpoint(&checkpoint, workflow) {
        return report(workflow, None, &state, "blocked", resumed, vec![error]);
    }
    checkpoint.usage.restore(&mut state);
    if checkpoint.in_flight {
        let note = format!("node `{}` has an uncertain execution outcome; reconcile its effects before starting a new runId", checkpoint.cursor);
        return report(workflow, Some(checkpoint), &state, "blocked", resumed, vec![note]);
    }
    if let Some(status) = checkpoint.terminal.clone() {
        return report(workflow, Some(checkpoint), &state, &status, resumed,
            vec!["replayed a recorded terminal result; no work or verification was repeated".to_string()]);
    }
    // Check evidence describes the workspace at its execution time. A later
    // process cannot assume the workspace (or an external system) stood still.
    if resumed { checkpoint.checks.clear(); }
    let mut notes = Vec::new();
    let iso = super::resolve_isolation(workflow.isolation, workflow.apply, opts.worktree, &mut notes);
    if workflow.isolation == crate::workflow_tools::spec::Isolation::Worktree && iso.is_none() {
        return report(workflow, Some(checkpoint), &state, "blocked", resumed,
            vec!["graph worktree isolation is unavailable".to_string()]);
    }
    let graph = workflow.graph.as_ref().expect("validated graph");
    let input_text = super::prompts::value_to_prompt_string(input);
    let status = loop {
        if super::is_cancelled(opts.cancel) {
            break "cancelled";
        }
        if checkpoint.steps >= graph.max_steps {
            notes.push(format!("graph step limit reached ({})", graph.max_steps));
            state.budget_exhausted = true;
            break "budget_exhausted";
        }
        let Some(node) = graph.nodes.iter().find(|node| node.id() == checkpoint.cursor) else {
            notes.push("checkpoint cursor does not name a graph node".to_string());
            break "blocked";
        };
        if matches!(node, GraphNode::Agent { .. }) && state.budget_blocks_spawn() {
            notes.extend(state.exhaustion_reason());
            state.budget_exhausted = true;
            break "budget_exhausted";
        }
        if matches!(node, GraphNode::Dream { .. }) && checkpoint.steps.saturating_add(2) > graph.max_steps {
            notes.push("graph has no step budget for dreaming and its completed end".to_string());
            state.budget_exhausted = true;
            break "budget_exhausted";
        }
        checkpoint.in_flight = true;
        if let Err(error) = checkpoint.save(options.store) {
            checkpoint.in_flight = false;
            notes.push(format!("graph did not start the next node: {error}"));
            break "blocked";
        }
        let intent = checkpoint.clone();
        if !matches!(node, GraphNode::Agent { .. }) {
            if let Some(sink) = opts.progress {
                sink.emit(super::ProgressEvent::PhaseEnter { id: node.id(), round: checkpoint.steps + 1 });
            }
        }
        let transition = execute(node, workflow, input, &input_text, backend, opts,
            &mut state, &mut checkpoint, iso);
        let transition = match transition {
            Ok(transition) => transition,
            Err(error) => {
                // The persisted intent stays unresolved. Even a command error
                // can mean it ran and its result was lost; never silently retry.
                notes.push(error);
                checkpoint = intent;
                break "blocked";
            }
        };
        if !matches!(node, GraphNode::Agent { .. }) {
            if let Some(sink) = opts.progress {
                let failed = usize::from(transition.outcome == "failed");
                sink.emit(super::ProgressEvent::PhaseDone {
                    id: node.id(), completed: 1 - failed, failed, still_running: 0,
                    carried: 0, retried: 0, skipped: 0,
                });
            }
        }
        checkpoint.steps += 1;
        checkpoint.visits.push(Visit {
            step: checkpoint.steps,
            node: node.id().to_string(),
            outcome: transition.outcome.to_string(),
            next: transition.next.clone(),
        });
        checkpoint.outputs.insert(node.id().to_string(), transition.output.clone());
        checkpoint.previous = transition.output;
        checkpoint.in_flight = false;
        checkpoint.terminal = transition.terminal.map(str::to_string);
        if let Some(next) = transition.next {
            checkpoint.cursor = next;
        }
        checkpoint.usage = Usage::capture(&state);
        if let Err(error) = checkpoint.save(options.store) {
            checkpoint = intent;
            notes.push(format!("graph result was not checkpointed; execution stopped: {error}"));
            break "blocked";
        }
        if let Some(status) = transition.terminal {
            break status;
        }
    };
    report(workflow, Some(checkpoint), &state, status, resumed, notes)
}

#[allow(clippy::too_many_arguments)]
fn execute(
    node: &GraphNode,
    workflow: &NormalizedWorkflow,
    input: &Value,
    input_text: &str,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions<'_>,
    state: &mut EngineState,
    checkpoint: &mut Checkpoint,
    iso: Option<super::spawn::IsolationCtx<'_>>,
) -> Result<Transition, String> {
    match node {
        GraphNode::Agent { id, phase, next, on_error } => {
            let mut phase_spec = workflow.phases.iter().find(|item| &item.id == phase)
                .expect("validated phase reference").clone();
            if let PhaseSource::Over(source) = &phase_spec.source {
                if !checkpoint.prior.contains_key(source) {
                    return Err(format!("node `{id}` needs phase `{source}`, which has not run"));
                }
            }
            if phase_spec.source == PhaseSource::Single {
                phase_spec.source = PhaseSource::Fanout(vec![checkpoint.previous.to_string()]);
            }
            phase_spec.id.clone_from(id);
            // Any agent may mutate the workspace. A prior green is no longer
            // evidence about this version, even if the agent calls itself a reader.
            checkpoint.checks.clear();
            let phase_report = super::run_phase(&phase_spec, input, input_text,
                &checkpoint.prior, backend, opts, state, iso);
            if let Some(sink) = opts.progress {
                super::emit_phase_done(sink, id, &phase_report);
            }
            let uncertain = phase_report.items.iter().any(|item| {
                matches!(item.status.as_str(), super::STATUS_STILL_RUNNING | super::STATUS_STOPPED)
                    || backend.worker_is_live(&item.agent_id)
            });
            let success = !phase_report.items.is_empty()
                && phase_report.items.iter().all(|item| item.status == super::STATUS_COMPLETED
                    && item.error.is_none() && (phase_spec.schema.is_none() || item.structured.is_some()));
            let output = json!({"items": &phase_report.items});
            let unmetered = phase_report.items.iter().any(|item| !item.agent_id.is_empty() && item.output_tokens == 0);
            checkpoint.prior.insert(phase.clone(), phase_report.items.clone());
            checkpoint.phases.push(phase_report);
            if unmetered && opts.graph.as_ref().is_some_and(|options| options.strict_accounting) {
                return Err(format!("node `{id}` has no trustworthy token usage; scheduled execution stopped"));
            }
            if uncertain || state.worktree_fallbacks > 0 || !state.apply_failures.is_empty() {
                return Err(format!("node `{id}` has unfinished work or an uncertain worktree result"));
            }
            if state.budget_exhausted {
                return Ok(Transition { output, outcome: "budget_exhausted", next: None, terminal: Some("budget_exhausted") });
            }
            Ok(Transition {
                output,
                outcome: if success { "completed" } else { "failed" },
                next: if success { Some(next.clone()) } else { on_error.clone() },
                terminal: (!success && on_error.is_none()).then_some("failed"),
            })
        }
        GraphNode::Check { id, command, on_pass, on_fail } => {
            let result = if let Some(check) = opts.graph.as_ref().and_then(|options| options.check) {
                check(command)
            } else {
                let check = opts.check.ok_or_else(|| "graph check runner is unavailable".to_string())?;
                CheckResult { exit_code: check(command), ..CheckResult::default() }
            };
            let exit_code = result.exit_code;

            if exit_code < 0 {
                return Err(format!("check node `{id}` did not produce an exit code"));
            }
            let passed = exit_code == 0;
            checkpoint.checks.insert(command.clone(), passed);
            Ok(Transition {
                output: json!({"command":command,"exit_code":exit_code,"passed":passed,"stdout":result.stdout,"stderr":result.stderr}),
                outcome: if passed { "passed" } else { "failed" },
                next: Some(if passed { on_pass } else { on_fail }.clone()),
                terminal: None,
            })
        }
        GraphNode::Branch { id, source, pointer, equals, on_match, on_miss } => {
            let value = checkpoint.outputs.get(source).and_then(|value| value.pointer(pointer))
                .ok_or_else(|| format!("branch `{id}` has no value at `{source}{pointer}`"))?;
            let matched = value == equals;
            Ok(Transition {
                output: json!({"matched":matched}),
                outcome: if matched { "matched" } else { "missed" },
                next: Some(if matched { on_match } else { on_miss }.clone()),
                terminal: None,
            })
        }
        GraphNode::Dream { next, .. } => {
            if !checkpoint.verified() {
                return Err("dream requires a passing check after the latest agent work and no unresolved red checks".to_string());
            }
            let options = opts.graph.as_ref().expect("graph options checked");
            let dream = options.dream.ok_or_else(|| "graph dreaming is unavailable".to_string())?;
            let checks: Vec<_> = checkpoint.checks.keys().cloned().collect();
            let output = dream(&checks, &checkpoint.session_id)?;
            Ok(Transition { output, outcome: "dreamed", next: Some(next.clone()), terminal: None })
        }
        GraphNode::End { status, .. } => {
            let status = match status { GraphEndStatus::Completed => "completed", GraphEndStatus::Failed => "failed" };
            Ok(Transition { output: json!({"status":status}), outcome: status, next: None, terminal: Some(status) })
        }
    }
}

fn report(
    workflow: &NormalizedWorkflow,
    checkpoint: Option<Checkpoint>,
    state: &EngineState,
    status: &str,
    resumed: bool,
    notes: Vec<String>,
) -> WorkflowReport {
    let accounting_complete = state.agents_spawned == 0 || checkpoint.as_ref().is_some_and(|checkpoint| {
        let items: Vec<_> = checkpoint.phases.iter().flat_map(|phase| &phase.items)
            .filter(|item| !item.agent_id.is_empty()).collect();
        items.len() == state.agents_spawned && items.iter().all(|item| item.output_tokens > 0)
    });
    let graph = checkpoint.as_ref().map(|checkpoint| json!({
        "accounting_complete":accounting_complete,
        "cursor":checkpoint.cursor, "steps":checkpoint.steps,
        "resumed":resumed, "verified":checkpoint.verified(),
        "verification_scope":"recorded_run",
        "in_flight":checkpoint.in_flight, "visits":checkpoint.visits,
        "outputs":checkpoint.outputs,
    }));
    WorkflowReport {
        graph, name: workflow.name.clone(), description: workflow.description.clone(),
        status: status.to_string(), phases: checkpoint.map_or_else(Vec::new, |c| c.phases),
        synthesis: None, judgement: None, agents_spawned: state.agents_spawned,
        budget_exhausted: state.budget_exhausted || status == "budget_exhausted", output_tokens: state.output_tokens_spent,
        cost_usd: state.cost_usd_spent, usage_cost: state.usage_cost.clone(),
        remaining_output_tokens: state.remaining_output_tokens(), remaining_cost_usd: state.remaining_cost_usd(),
        notes,
    }
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
