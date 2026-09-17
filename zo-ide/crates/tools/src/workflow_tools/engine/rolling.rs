use std::collections::HashMap;
use std::time::Instant;

use super::super::progress::ProgressEvent;
use super::super::wait::WaitState;
use super::items::assemble_item;
use super::prompts::phase_agent_input;
use super::spawn::{SpawnUnit, Spawned};
use super::{
    is_cancelled, phase_hard_timeout_from_env, AgentBackend, AgentCompletion, EngineState,
    ItemResult, NormalizedPhase, RunOptions, STATUS_COMPLETED, STATUS_FAILED, STATUS_STOPPED,
};

fn terminal(completion: &AgentCompletion) -> bool {
    matches!(completion.status.as_str(), STATUS_COMPLETED | STATUS_FAILED | STATUS_STOPPED)
}

fn collect_item(
    phase: &NormalizedPhase,
    spawned: Spawned,
    completion: AgentCompletion,
    backend: &dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
) -> ItemResult {
    state.record_usage(completion.run.output_tokens, backend.usage_cost(&completion.agent_id));
    if let Some(sink) = opts.progress {
        sink.emit(ProgressEvent::AgentDone {
            phase_id: &phase.id,
            agent_id: &completion.agent_id,
            status: &completion.status,
        });
    }
    let skills = backend.activity(&completion.agent_id)
        .map_or_else(Vec::new, |activity| activity.loaded_skills);
    assemble_item(spawned, &[completion], phase.schema.as_ref(), skills)
}

pub(super) fn spawn_and_collect(
    phase: &NormalizedPhase,
    units: Vec<SpawnUnit>,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    window: usize,
) -> Vec<ItemResult> {
    let mut units = units.into_iter().peekable();
    let mut active: Vec<Spawned> = Vec::with_capacity(window);
    let mut observed: HashMap<String, AgentCompletion> = HashMap::new();
    let mut items = Vec::new();
    let mut watch = WaitState::new(Instant::now(), phase_hard_timeout_from_env(opts.phase_timeout));

    loop {
        while active.len() < window && units.peek().is_some() {
            if watch.next_slice(Instant::now(), is_cancelled(opts.cancel)).is_err() {
                state.spawn_block = Some("read-only workflow collection stopped; further spawns blocked");
                break;
            }
            if !observed.is_empty()
                && (state.max_cost_usd.is_some() || state.max_output_tokens.is_some())
            {
                // A logically stopped worker can still add usage before physical exit.
                break;
            }
            if state.budget_blocks_spawn() {
                state.mark_exhausted();
                break;
            }
            let Some(unit) = units.next() else { break };
            let input = phase_agent_input(
                phase, unit.index, &unit.item, unit.prompt, unit.prior_failures, false, None,
            );
            let outcome = backend.spawn(input).map_err(|error| error.to_string());
            let spawned = Spawned { index: unit.index, item: unit.item, outcome };
            if let Ok(id) = &spawned.outcome {
                state.record_spawn();
                if let Some(sink) = opts.progress {
                    sink.emit(ProgressEvent::AgentsSpawned {
                        phase_id: &phase.id,
                        agent_ids: std::slice::from_ref(id),
                    });
                }
                active.push(spawned);
            } else {
                items.push(assemble_item(spawned, &[], phase.schema.as_ref(), Vec::new()));
            }
        }
        if active.is_empty() {
            break;
        }

        let ids: Vec<String> = active.iter()
            .filter_map(|spawned| spawned.outcome.as_ref().ok().cloned()).collect();
        for completion in backend.wait_next(&ids, opts.phase_timeout, &mut watch) {
            if terminal(&completion) && ids.contains(&completion.agent_id) {
                // A worker's cancellation receipt must not replace watchdog salvage.
                observed.entry(completion.agent_id.clone())
                    .and_modify(|held| {
                        held.run.output_tokens = held.run.output_tokens.max(completion.run.output_tokens);
                    })
                    .or_insert(completion);
            }
        }
        let stopping = watch.next_slice(Instant::now(), is_cancelled(opts.cancel)).is_err();
        if stopping {
            state.spawn_block = Some("read-only workflow collection stopped; further spawns blocked");
        }
        let mut pending = Vec::with_capacity(active.len());
        for spawned in active {
            let Ok(id) = &spawned.outcome else { continue };
            let exited = !backend.worker_is_live(id);
            let completion = if stopping && (!exited || !observed.contains_key(id)) {
                let cancelled = backend.cancel(id);
                observed.remove(id).or_else(|| cancelled.map(|mut completion| {
                    completion.status = STATUS_STOPPED.to_string();
                    completion.error = Some("worker exit unconfirmed when workflow collection stopped".to_string());
                    completion
                }))
            } else if exited {
                observed.remove(id)
            } else {
                None
            };
            if let Some(completion) = completion {
                items.push(collect_item(phase, spawned, completion, backend, opts, state));
            } else if stopping {
                items.push(assemble_item(spawned, &[], phase.schema.as_ref(), Vec::new()));
            } else {
                pending.push(spawned);
            }
        }
        active = pending;
        if stopping {
            break;
        }
    }
    items.sort_by_key(|item| item.index);
    items
}
