//! Phase spawning and scheduling: isolation resolution, per-item agent
//! spawn units, completion accounting, worktree merge-back, and rounds.
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use core_types::helper_run::HelperRun;
use serde_json::Value;

use crate::misc_tools::{AgentCompletion, AgentInput};

use super::super::progress::{ProgressEvent, ProgressSink};
use super::super::spec::{ApplyPolicy, Isolation, NormalizedPhase, RepeatPolicy, RepairStep, Until};
use super::super::worktree::{WorktreeGuard, WorktreeProvider};
use super::attribution;
use super::items::{
    accumulate_round_with_folds, assemble_item, normalize_semantic_verdict, resolve_items,
    schema_retry_pass,
    semantic_verdict, seen_text, FindingEvidence,
};
use super::prompts::{phase_agent_input, render_fixer_prompt, render_prompt, render_reverify_prompt};
use super::{
    is_cancelled, AgentBackend, EngineState, Finding, FindingState, ItemResult, PassReceipt,
    PhaseReport, Risk, RunOptions, SemanticCacheKey, SemanticVerdict,
    PHASE_TIMEOUT_STOP_ERROR, STATUS_COMPLETED, STATUS_FAILED, STATUS_STILL_RUNNING,
    STATUS_STOPPED,
};

/// Resolved per-run isolation: the provider that hands out per-agent worktrees,
/// plus whether each agent's changes are merged back into the main tree after
/// the barrier (`apply:"sequential"`). `Copy` so it threads as `Option<_>`
/// alongside `&RunOptions` with no `&mut` plumbing, exactly like the bare
/// provider reference it replaces.
#[derive(Clone, Copy)]
pub(super) struct IsolationCtx<'a> {
    pub(super) provider: &'a dyn WorktreeProvider,
    pub(super) merge_back: bool,
}

/// Decide whether per-agent worktree isolation is active for this run and push
/// an honest note for the outcome. Returns the isolation context to use, or
/// `None` to run without isolation. Both modes call it once, before the loop.
pub(super) fn resolve_isolation<'a>(
    isolation: Isolation,
    apply: ApplyPolicy,
    provider: Option<&'a dyn WorktreeProvider>,
    notes: &mut Vec<String>,
) -> Option<IsolationCtx<'a>> {
    if isolation != Isolation::Worktree {
        return None;
    }
    let Some(provider) = provider else {
        notes.push(
            "isolation \"worktree\" requested but no worktree provider was available (not a git work tree?); ran without isolation".to_string(),
        );
        return None;
    };
    let merge_back = apply == ApplyPolicy::Sequential;
    if merge_back {
        notes.push(
            "isolation \"worktree\" + apply \"sequential\": each agent ran in its own git worktree; their changes are merged back into the working tree (git apply --3way, in spawn order) after each batch".to_string(),
        );
    } else {
        notes.push(
            "isolation \"worktree\": each agent ran in its own git worktree (removed afterward; changes are not merged back)".to_string(),
        );
    }
    Some(IsolationCtx {
        provider,
        merge_back,
    })
}

/// One spawned agent within a phase: either launched (`Ok(id)`) or rejected at
/// spawn time (`Err(message)`), plus the rendered item it owns.
pub(super) struct Spawned {
    pub(super) index: usize,
    pub(super) item: String,
    pub(super) outcome: Result<String, String>,
}

#[allow(clippy::too_many_arguments)] // orchestration: phase + ambient run deps
pub(super) fn run_phase(
    phase: &NormalizedPhase,
    input: &Value,
    input_text: &str,
    prior: &HashMap<String, Vec<ItemResult>>,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
) -> PhaseReport {
    // Per-phase token accounting is a snapshot-delta of the single run-cumulative
    // tally, so it needs no extra plumbing through `wait_and_account`.
    let tokens_before = state.output_tokens_spent;
    // Fan-out items are stable across rounds; only `{seen}` changes per round.
    let base_items = resolve_items(&phase.source, input, prior);
    if phase.repair_loop.is_some() {
        return run_fix_until_verified(phase, input_text, base_items, backend, opts, state, iso, tokens_before);
    }
    let max_rounds = phase.repeat.as_ref().map_or(1, |policy| policy.max_rounds);

    let mut accumulated: Vec<ItemResult> = Vec::new();
    // Escalation counts actual quality attempts independently of report dedup.
    let mut quality_failures_by_item: HashMap<usize, u32> = HashMap::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut totals = RepeatTotals::default();
    let mut command_green_passed = false;
    let selective_repeat = selective_repeat_enabled(phase);
    if selective_repeat {
        seed_semantic_cache_passes(phase, input_text, &base_items, opts, &mut accumulated);
    }

    for round in 0..max_rounds {
        if stop_before_round(round, opts, state) {
            break;
        }

        let mut round_work = select_round_work(
            selective_repeat,
            &base_items,
            &mut accumulated,
            &quality_failures_by_item,
            &mut totals.carried_pass_count,
            &mut totals.retried_finding_count,
            &mut totals.skipped_count,
        );
        let clamped = clamp_round_work_to_budget(&mut round_work, state);
        if clamped {
            state.mark_exhausted();
        }
        if round_work.is_empty() {
            if let Some(outcome) = command_green_outcome_for_current_tree(
                phase,
                opts,
                selective_repeat,
                &base_items,
                &accumulated,
            ) {
                command_green_passed = outcome.passed;
            }
            break;
        }

        if let Some(sink) = opts.progress {
            sink.emit(ProgressEvent::PhaseEnter {
                id: &phase.id,
                round: round + 1,
            });
        }

        let seen = seen_text(&accumulated);
        totals.relay_bytes = totals.relay_bytes.saturating_add(observe_relay(phase, &seen));
        let round_items = run_round(phase, input_text, &round_work, &seen, backend, opts, state, iso);
        let completed_indices = record_round_outcome(&round_items, selective_repeat, &mut quality_failures_by_item);
        totals.rounds_run += 1;
        let (new_in_round, folded_lanes) = accumulate_round_with_folds(
            &mut accumulated,
            &mut seen_keys,
            round_items,
            phase.repeat.as_ref().and_then(|policy| policy.dedup_by.as_deref()),
        );
        // Each lane the union folded away as a duplicate is a wasted spawn —
        // recorded as a FOLD decision against its model (design P1 `fold`), the
        // signal the accuracy report's fold rate reads.
        for agent_id in &folded_lanes {
            attribution::record_fold_outcome_for_agent(backend.agent_registry().as_deref(), agent_id);
        }
        if selective_repeat {
            store_semantic_cache_passes(phase, input_text, opts, &accumulated);
        }

        if state.budget_exhausted {
            break;
        }
        // `until: no_new` — stop once a round contributes nothing new.
        if matches!(phase.repeat.as_ref(), Some(policy) if policy.until == Until::NoNew)
            && new_in_round == 0
        {
            break;
        }
        if command_green_after_round(
            phase,
            opts,
            backend.agent_registry().as_deref(),
            selective_repeat,
            &base_items,
            &accumulated,
            &completed_indices,
            &mut quality_failures_by_item,
        ) {
            command_green_passed = true;
            break;
        }
    }

    record_command_green_note(phase, state, totals.rounds_run, command_green_passed);

    renumber_accumulated_items(&mut accumulated);

    totals.output_tokens = state.output_tokens_spent.saturating_sub(tokens_before);
    build_repeat_phase_report(phase, accumulated, totals)
}

fn stop_before_round(round: u32, opts: &RunOptions, state: &mut EngineState) -> bool {
    // Every phase may run its first round. Later rounds observe cooperative
    // cancellation and the budget consumed by preceding attempts.
    if round == 0 {
        return false;
    }
    if is_cancelled(opts.cancel) {
        return true;
    }
    if state.output_tokens_exhausted() || state.cost_exhausted() {
        state.mark_exhausted();
        return true;
    }
    false
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines, clippy::needless_pass_by_value)] // repair orchestration threads engine ambient deps
fn run_fix_until_verified(
    phase: &NormalizedPhase,
    input_text: &str,
    base_items: Vec<String>,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
    tokens_before: u64,
) -> PhaseReport {
    let repair_loop = phase.repair_loop.as_ref().expect("repair loop checked by caller");
    let mut accumulated = Vec::new();
    let mut findings = Vec::new();
    let mut pass_receipts = Vec::new();
    let mut blocked_finding_count = 0usize;
    let mut escalated_finding_count = 0usize;
    let mut retried_finding_count = 0usize;
    let mut carried_pass_count = 0usize;

    let mut round_work = indexed_items(&base_items);
    if clamp_round_work_to_budget(&mut round_work, state) {
        state.mark_exhausted();
    }
    if let Some(sink) = opts.progress {
        sink.emit(ProgressEvent::PhaseEnter { id: &phase.id, round: 1 });
    }
    if !round_work.is_empty() {
        let initial = run_round(phase, input_text, &round_work, "", backend, opts, state, iso);
        classify_validator_items(
            backend.agent_registry().as_deref(),
            phase,
            &initial,
            opts.progress,
            &mut findings,
            &mut pass_receipts,
            &mut carried_pass_count,
        );
        accumulated.extend(initial);
    }

    let mut attempts = 0u32;
    while attempts < repair_loop.max_attempts && findings.iter().any(is_actionable_finding) {
        if is_cancelled(opts.cancel) || state.budget_exhausted {
            break;
        }
        attempts += 1;
        let actionable: Vec<Finding> = findings.iter().filter(|finding| is_actionable_finding(finding)).cloned().collect();
        for finding in actionable {
            if state.budget_blocks_spawn() || state.output_tokens_exhausted() || state.cost_exhausted() {
                state.mark_exhausted();
                mark_finding_state(&mut findings, &finding.id, FindingState::Escalated, attempts as usize);
                escalated_finding_count = escalated_finding_count.saturating_add(1);
                continue;
            }
            if let Some(sink) = opts.progress {
                sink.emit(ProgressEvent::SelectiveRetryStarted {
                    phase_id: &phase.id,
                    finding_id: &finding.id,
                });
            }
            mark_finding_state(&mut findings, &finding.id, FindingState::Fixing, attempts as usize);
            let fixer_item = run_repair_step(
                phase,
                &repair_loop.fixer,
                &finding,
                &pass_receipts,
                backend,
                opts,
                state,
                iso,
                accumulated.len(),
            );
            let fixer_agent_id = fixer_item.agent_id.clone();
            let changed_paths = changed_paths_for_invalidation(&fixer_item, &finding);
            invalidate_pass_receipts_for_changed_paths(
                phase,
                &changed_paths,
                &mut pass_receipts,
                opts.progress,
                attempts as usize,
            );
            invalidate_pass_receipts_for_finding_risk(
                phase,
                finding.risk,
                &mut pass_receipts,
                opts.progress,
                attempts as usize,
            );
            accumulated.push(fixer_item);

            if is_cancelled(opts.cancel) || state.budget_exhausted {
                break;
            }
            mark_finding_state(&mut findings, &finding.id, FindingState::Verifying, attempts as usize);
            if let Some(sink) = opts.progress {
                sink.emit(ProgressEvent::ItemInvalidated {
                    phase_id: &phase.id,
                    item_index: attempts as usize,
                    reason: "fixer changed the finding context; focused reverify required",
                });
            }
            // The reverify judges THIS fixer attempt: bind it now, before the
            // reverify runs, so its verdict names this attempt and the model
            // and route it ran on even if the fixer is resumed meanwhile.
            let fixer_attempt =
                attribution::bind_attempt(backend.agent_registry().as_deref(), &fixer_agent_id);
            let reverify = run_focused_reverify(
                phase,
                repair_loop.validator.as_ref(),
                &finding,
                input_text,
                backend,
                opts,
                state,
                iso,
                accumulated.len(),
            );
            let reverify_agent_id = reverify.agent_id.clone();
            let verdict = normalize_semantic_verdict(
                reverify.index,
                &reverify.input,
                &reverify.status,
                reverify.structured.as_ref(),
            );
            accumulated.push(reverify);
            retried_finding_count = retried_finding_count.saturating_add(1);
            match verdict {
                SemanticVerdict::Pass(receipt) => {
                    // P2 verdict attribution: the focused reverify judged the
                    // FIXER's change good — one decisive quality sample for
                    // the fixer's routed model.
                    attribution::record_verdict_outcome_for_attempt(
                        fixer_attempt.as_ref(),
                        true,
                        attribution::VerdictKind::PassFail,
                        runtime::VerdictBasis::Model,
                        // The reverify's tree is not one this road names.
                        None,
                    );
                    if let Some(sink) = opts.progress {
                        sink.emit(ProgressEvent::ItemCarried {
                            phase_id: &phase.id,
                            item_index: receipt.item_index,
                        });
                    }
                    pass_receipts.push(receipt);
                    mark_finding_state(&mut findings, &finding.id, FindingState::Fixed, attempts as usize);
                }
                SemanticVerdict::Finding(next)
                    if same_finding_evidence(&next, &finding)
                        || attempts >= repair_loop.max_attempts =>
                {
                    let repeated = same_finding_evidence(&next, &finding);
                    if repeated {
                        // The reverify reproduced the SAME finding — the fixer
                        // demonstrably failed to fix it. (A different finding
                        // at the attempt cap has ambiguous provenance and
                        // records nothing.)
                        attribution::record_verdict_outcome_for_attempt(
                            fixer_attempt.as_ref(),
                            false,
                            attribution::VerdictKind::PassFail,
                            runtime::VerdictBasis::Model,
                            // The reverify's tree is not one this road names.
                            None,
                        );
                    }
                    let reason = if repeated {
                        "repeated same finding"
                    } else {
                        "max repair attempts reached"
                    };
                    if let Some(sink) = opts.progress {
                        sink.emit(ProgressEvent::FindingBlocked {
                            phase_id: &phase.id,
                            finding_id: &finding.id,
                            reason,
                        });
                    }
                    mark_finding_state(&mut findings, &finding.id, FindingState::Blocked, attempts as usize);
                    blocked_finding_count = blocked_finding_count.saturating_add(1);
                }
                SemanticVerdict::Finding(next) => {
                    if !findings.iter().any(|seen| seen.id == next.id) {
                        if let Some(sink) = opts.progress {
                            sink.emit(ProgressEvent::FindingQueued {
                                phase_id: &phase.id,
                                finding_id: &next.id,
                            });
                        }
                        findings.push(next);
                    }
                }
                SemanticVerdict::Invalid(_) => {
                    // Unusable reverify output is a quality failure of the
                    // VALIDATOR's own model (this "failed" verdict speaks for
                    // its attempt instead of that run's bare "completed") —
                    // stamped as the validator's fault so it is never read as
                    // a caught defect. The FIXER's change went unjudged: its
                    // attempt is unverified, which costs it nothing.
                    attribution::record_verdict_outcome_for_agent(
                        backend.agent_registry().as_deref(),
                        &reverify_agent_id,
                        false,
                        attribution::VerdictKind::ValidatorFault,
                    runtime::VerdictBasis::Model,
                    );
                    attribution::record_verification_unavailable_for_attempt(fixer_attempt.as_ref());
                    if attempts >= repair_loop.max_attempts {
                        mark_finding_state(&mut findings, &finding.id, FindingState::Blocked, attempts as usize);
                        blocked_finding_count = blocked_finding_count.saturating_add(1);
                    }
                }
                SemanticVerdict::Unknown => {
                    // The reverify never settled (it did not finish, or said
                    // neither pass nor fail): the fixer's attempt is
                    // unverified — no win for finishing, no blame.
                    attribution::record_verification_unavailable_for_attempt(fixer_attempt.as_ref());
                    if attempts >= repair_loop.max_attempts {
                        mark_finding_state(&mut findings, &finding.id, FindingState::Blocked, attempts as usize);
                        blocked_finding_count = blocked_finding_count.saturating_add(1);
                    }
                }
            }
        }
    }

    let final_green = opts.check.is_some_and(|check| check(&repair_loop.final_check.command) == 0);
    if !final_green {
        let final_id = format!("final-check:{}", phase.id);
        if let Some(sink) = opts.progress {
            sink.emit(ProgressEvent::FindingBlocked {
                phase_id: &phase.id,
                finding_id: &final_id,
                reason: "final check did not pass",
            });
        }
        findings.push(Finding {
            id: final_id,
            title: "final check failed".to_string(),
            affected_paths: Vec::new(),
            risk: Risk::Global,
            evidence: repair_loop.final_check.command.clone(),
            state: FindingState::Blocked,
            attempts: attempts as usize,
        });
        blocked_finding_count = blocked_finding_count.saturating_add(1);
    }

    PhaseReport {
        id: phase.id.clone(),
        rounds: attempts.saturating_add(1),
        items: accumulated,
        carried_pass_count,
        retried_finding_count,
        skipped_count: 0,
        blocked_finding_count,
        escalated_finding_count,
        findings,
        pass_receipts,
        output_tokens: state.output_tokens_spent.saturating_sub(tokens_before),
        // A repair loop is a retry of the SAME item, not a later round over an
        // accumulator, so there is no prior-round relay to carry here.
        relay_bytes: 0,
    }
}

fn classify_validator_items(
    registry: Option<&crate::misc_tools::AgentRegistry>,
    phase: &NormalizedPhase,
    items: &[ItemResult],
    sink: Option<&dyn ProgressSink>,
    findings: &mut Vec<Finding>,
    pass_receipts: &mut Vec<PassReceipt>,
    carried_pass_count: &mut usize,
) {
    for item in items {
        match normalize_semantic_verdict(item.index, &item.input, &item.status, item.structured.as_ref()) {
            SemanticVerdict::Pass(receipt) => {
                *carried_pass_count = carried_pass_count.saturating_add(1);
                if let Some(sink) = sink {
                    sink.emit(ProgressEvent::ItemCarried { phase_id: &phase.id, item_index: item.index });
                }
                pass_receipts.push(receipt);
            }
            SemanticVerdict::Finding(finding) => {
                if let Some(sink) = sink {
                    sink.emit(ProgressEvent::FindingQueued { phase_id: &phase.id, finding_id: &finding.id });
                }
                if findings.iter().all(|seen| seen.id != finding.id) {
                    findings.push(finding);
                }
            }
            SemanticVerdict::Invalid(reason) => {
                // P2 verdict attribution: unusable validator output is a
                // quality failure of the VALIDATOR's own model (the item here
                // IS the validator's run, so the binding is exact) — its own
                // fault, like the focused reverify's, never a defect caught in
                // the work it was sweeping.
                attribution::record_verdict_outcome_for_agent(
                    registry,
                    &item.agent_id,
                    false,
                    attribution::VerdictKind::ValidatorFault,
                runtime::VerdictBasis::Model,
                );
                findings.push(Finding {
                    id: format!("invalid:{}:{}", phase.id, item.index),
                    title: "invalid validator output".to_string(),
                    affected_paths: Vec::new(),
                    risk: Risk::Local,
                    evidence: reason,
                    state: FindingState::Queued,
                    attempts: 0,
                });
            }
            SemanticVerdict::Unknown => {}
        }
    }
}

fn is_actionable_finding(finding: &Finding) -> bool {
    matches!(finding.state, FindingState::Queued | FindingState::Verifying | FindingState::Fixing)
}

/// Whether a focused-reverify finding repeats the one we just tried to fix —
/// by [`FindingEvidence`], the one "same problem" identity the verify-by-default
/// loop shares.
fn same_finding_evidence(a: &Finding, b: &Finding) -> bool {
    FindingEvidence::of(a) == FindingEvidence::of(b)
}

fn mark_finding_state(findings: &mut [Finding], finding_id: &str, state: FindingState, attempts: usize) {
    if let Some(finding) = findings.iter_mut().find(|finding| finding.id == finding_id) {
        finding.state = state;
        finding.attempts = attempts;
    }
}

fn pass_receipts_text(receipts: &[PassReceipt]) -> String {
    receipts
        .iter()
        .map(|receipt| format!("{}: {}", receipt.receipt_key, receipt.coverage))
        .collect::<Vec<_>>()
        .join("\n")
}

fn changed_paths_for_invalidation(item: &ItemResult, finding: &Finding) -> Vec<String> {
    let mut paths = item
        .structured
        .as_ref()
        .map_or_else(Vec::new, structured_paths);
    if paths.is_empty() {
        paths.extend(finding.affected_paths.iter().cloned());
    }
    paths.sort();
    paths.dedup();
    paths
}

fn structured_paths(value: &Value) -> Vec<String> {
    ["changed_paths", "affected_paths", "paths"]
        .into_iter()
        .filter_map(|key| value.get(key))
        .flat_map(value_paths)
        .collect()
}

fn value_paths(value: &Value) -> Vec<String> {
    match value {
        Value::String(path) => clean_path(path).into_iter().collect(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .filter_map(clean_path)
            .collect(),
        _ => Vec::new(),
    }
}

fn clean_path(path: &str) -> Option<String> {
    let path = path.trim().trim_matches(['`', '"', '\'']);
    (!path.is_empty()).then_some(path.to_string())
}

fn invalidate_pass_receipts_for_changed_paths(
    phase: &NormalizedPhase,
    changed_paths: &[String],
    receipts: &mut Vec<PassReceipt>,
    sink: Option<&dyn ProgressSink>,
    item_index: usize,
) {
    if changed_paths.is_empty() || receipts.is_empty() {
        return;
    }
    let before = receipts.len();
    receipts.retain(|receipt| {
        !changed_paths
            .iter()
            .any(|path| receipt.coverage.contains(path) || receipt.receipt_key.contains(path))
    });
    if receipts.len() != before {
        if let Some(sink) = sink {
            sink.emit(ProgressEvent::ItemInvalidated {
                phase_id: &phase.id,
                item_index,
                reason: "fixer changed a previously covered path; cached pass receipt invalidated",
            });
        }
    }
}

/// Broad invalidation by the fixed finding's blast radius (safety rule #6).
/// A change to a shared/global surface can affect passes that do not directly
/// overlap the changed paths, so a `Global` fix invalidates every carried
/// receipt and a `Shared` fix invalidates other shared/global receipts. A
/// `Local` fix relies solely on path-overlap invalidation. Risk is the
/// validator's declared evidence, not a hardcoded path list.
fn invalidate_pass_receipts_for_finding_risk(
    phase: &NormalizedPhase,
    finding_risk: Risk,
    receipts: &mut Vec<PassReceipt>,
    sink: Option<&dyn ProgressSink>,
    item_index: usize,
) {
    if receipts.is_empty() {
        return;
    }
    let before = receipts.len();
    match finding_risk {
        Risk::Global => receipts.clear(),
        Risk::Shared => receipts.retain(|receipt| receipt.risk.is_local()),
        Risk::Local => {}
    }
    if receipts.len() != before {
        if let Some(sink) = sink {
            sink.emit(ProgressEvent::ItemInvalidated {
                phase_id: &phase.id,
                item_index,
                reason: "shared/global-risk fix invalidated broader pass receipts",
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_repair_step(
    phase: &NormalizedPhase,
    step: &RepairStep,
    finding: &Finding,
    pass_receipts: &[PassReceipt],
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
    index: usize,
) -> ItemResult {
    let prompt = render_fixer_prompt(
        step,
        &finding.id,
        &finding.title,
        &finding.affected_paths,
        &finding.evidence,
        &pass_receipts_text(pass_receipts),
    );
    run_single_repair_agent(phase, step, finding, prompt, backend, opts, state, iso, index)
}

#[allow(clippy::too_many_arguments)]
fn run_focused_reverify(
    phase: &NormalizedPhase,
    validator: Option<&RepairStep>,
    finding: &Finding,
    input_text: &str,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
    index: usize,
) -> ItemResult {
    let schema = validator.and_then(|step| step.schema.as_ref()).or(phase.schema.as_ref());
    let template = validator.map_or(phase.prompt.as_str(), |step| step.prompt.as_str());
    let prompt = render_reverify_prompt(
        template,
        &finding.id,
        &finding.title,
        &finding.evidence,
        input_text,
        schema,
    );
    let step = RepairStep {
        prompt: template.to_string(),
        subagent_type: validator
            .and_then(|step| step.subagent_type.clone())
            .or_else(|| phase.subagent_type.clone()),
        model: validator
            .and_then(|step| step.model.clone())
            .or_else(|| phase.model.clone()),
        schema: schema.cloned(),
    };
    run_single_repair_agent(phase, &step, finding, prompt, backend, opts, state, iso, index)
}

fn repair_agent_cwd(
    phase: &NormalizedPhase,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
    guards: &mut Vec<Box<dyn WorktreeGuard>>,
) -> Option<std::path::PathBuf> {
    iso.and_then(|ctx| {
        if let Ok(guard) = ctx.provider.create(&phase.id) {
            if let Some(warning) = guard.creation_warning() {
                state.record_worktree_warning(warning);
            }
            let path = guard.path().to_path_buf();
            guards.push(guard);
            Some(path)
        } else {
            state.record_worktree_fallback();
            None
        }
    })
}

fn repair_agent_input(
    phase: &NormalizedPhase,
    step: &RepairStep,
    finding: &Finding,
    prompt: String,
    cwd: Option<std::path::PathBuf>,
) -> AgentInput {
    let model = step.model.clone().or_else(|| phase.model.clone());
    AgentInput {
        route_probe_confidence: None,
        fork_source: None,
        allow_cross_provider: model.is_some(),
        description: format!("workflow phase `{}` finding `{}`", phase.id, finding.id),
        prompt,
        subagent_type: step
            .subagent_type
            .clone()
            .or_else(|| phase.subagent_type.clone()),
        name: Some(format!("{}:{}", phase.id, finding.id)),
        addressable_name: None,
        model,
        cwd,
        schema: step.schema.clone(),
        workflow_member: true,
        one_shot: false,
        plan_shape: None,
        route_tax: None,
        background: Some(false),
        // Stamped by the backend spawn seam (`LiveBackend::spawn`); the
        // engine-side literal stays neutral.
        parent_permission_mode: None,
        parent_session_id: None,
        registry: None,
        tool_call_id: None,
        mcp_passthrough: None,
        api_concurrency: None,
        time_budget: None,
        prior_failures: 0,
        route_reason: None,
        route_role: None,
        route_complexity: None,
        route_risk: None,
        route_source: None,
        route_model: None,
        route_fallback_models: Vec::new(),
        route_effort: None,
        judged_agent: None,
        verify_loop: None,
        sees: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_single_repair_agent(
    phase: &NormalizedPhase,
    step: &RepairStep,
    finding: &Finding,
    prompt: String,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
    index: usize,
) -> ItemResult {
    let mut guards: Vec<Box<dyn WorktreeGuard>> = Vec::new();
    let cwd = repair_agent_cwd(phase, state, iso, &mut guards);
    let input = repair_agent_input(phase, step, finding, prompt, cwd);
    let Ok(agent_id) = backend.spawn(input) else {
        return ItemResult {
            index,
            input: finding.title.clone(),
            agent_id: String::new(),
            status: STATUS_FAILED.to_string(),
            result: None,
            error: Some("repair agent spawn failed".to_string()),
            structured: None,
            output_tokens: 0,
            loaded_skills: Vec::new(),
            semantic_verdict: Some("spawn_error".to_string()),
            retry_key: Some(finding.id.clone()),
            carry_reason: None,
            carried: false,
        };
    };
    state.record_spawn();
    let completion = wait_and_account(backend, state, std::slice::from_ref(&agent_id), opts.phase_timeout)
        .into_iter()
        .find(|completion| completion.agent_id == agent_id)
        .unwrap_or_else(|| AgentCompletion {
            agent_id: agent_id.clone(),
            name: String::new(),
            status: STATUS_STILL_RUNNING.to_string(),
            result: None,
            error: Some("repair agent did not finish before timeout".to_string()),
            structured: None,
            run: HelperRun::default(),
        });
    if let Some(ctx) = iso.filter(|ctx| ctx.merge_back) {
        merge_back_batch(&guards, ctx.provider, &phase.id, state);
    }
    let loaded_skills = backend
        .activity(&agent_id)
        .map_or_else(Vec::new, |activity| activity.loaded_skills);
    assemble_item(
        Spawned {
            index,
            item: finding.title.clone(),
            outcome: Ok(agent_id),
        },
        &[completion],
        step.schema.as_ref(),
        loaded_skills,
    )
}

/// One unit of work for [`spawn_and_collect`]: the rendered prompt plus the
/// identity (`index`) and `{item}` text to record on the resulting
/// [`ItemResult`].
#[derive(Debug, Clone)]
pub(super) struct RoundWorkItem {
    pub(super) index: usize,
    pub(super) item: String,
    pub(super) prior_failures: u32,
}

struct RetrySelection {
    items: Vec<RoundWorkItem>,
    carried: usize,
    retried: usize,
    skipped: usize,
}

fn indexed_items(items: &[String]) -> Vec<RoundWorkItem> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| RoundWorkItem {
            index,
            item: item.clone(),
            prior_failures: 0,
        })
        .collect()
}

fn selective_repeat_enabled(phase: &NormalizedPhase) -> bool {
    phase.schema.is_some()
        && matches!(
            phase.repeat.as_ref().map(|policy| &policy.until),
            Some(Until::CommandGreen { .. })
        )
}

fn renumber_accumulated_items(items: &mut [ItemResult]) {
    for (index, item) in items.iter_mut().enumerate() {
        item.index = index;
    }
}

/// Record what this round's relay carried, and return its size.
///
/// Two observation surfaces, deliberately: the ledger answers "did the
/// injection fire at all" (the question a shipped-but-inert feature fails), and
/// the returned byte count becomes a durable field on the run record. The
/// authored-`{seen}` case is a DECLINE, not a miss — the relay is present,
/// the engine simply had nothing to add.
fn observe_relay(phase: &NormalizedPhase, seen: &str) -> u64 {
    if seen.is_empty() {
        return 0;
    }
    if phase.prompt.contains("{seen}") {
        telemetry::attest_declined(
            telemetry::HarnessFeature::WorkflowRelay,
            "author_placed_seen",
        );
    } else {
        telemetry::attest_fired(telemetry::HarnessFeature::WorkflowRelay);
    }
    seen.len() as u64
}

/// What a repeat phase accumulated across its rounds, apart from the items
/// themselves. Grouped rather than passed one by one: they are all counters
/// over the same loop, and threading each as its own parameter is what turned
/// adding one observation into a signature change.
#[derive(Default, Clone, Copy)]
struct RepeatTotals {
    carried_pass_count: usize,
    retried_finding_count: usize,
    skipped_count: usize,
    output_tokens: u64,
    rounds_run: u32,
    relay_bytes: u64,
}

fn build_repeat_phase_report(
    phase: &NormalizedPhase,
    items: Vec<ItemResult>,
    totals: RepeatTotals,
) -> PhaseReport {
    let pass_receipts = collect_pass_receipts(&items);
    PhaseReport {
        id: phase.id.clone(),
        rounds: totals.rounds_run.max(1),
        relay_bytes: totals.relay_bytes,
        items,
        carried_pass_count: totals.carried_pass_count,
        retried_finding_count: totals.retried_finding_count,
        skipped_count: totals.skipped_count,
        blocked_finding_count: 0,
        escalated_finding_count: 0,
        findings: Vec::new(),
        pass_receipts,
        output_tokens: totals.output_tokens,
    }
}

fn select_round_work(
    selective_repeat: bool,
    base_items: &[String],
    accumulated: &mut [ItemResult],
    quality_failures_by_item: &HashMap<usize, u32>,
    carried_pass_count: &mut usize,
    retried_finding_count: &mut usize,
    skipped_count: &mut usize,
) -> Vec<RoundWorkItem> {
    if !selective_repeat {
        let mut items = indexed_items(base_items);
        for item in &mut items {
            item.prior_failures = quality_failures_by_item
                .get(&item.index)
                .copied()
                .unwrap_or(0);
        }
        return items;
    }
    let selection = select_retry_work(base_items, accumulated, quality_failures_by_item);
    *carried_pass_count += selection.carried;
    *retried_finding_count += selection.retried;
    *skipped_count += selection.skipped;
    selection.items
}

fn record_command_green_note(
    phase: &NormalizedPhase,
    state: &mut EngineState,
    rounds_run: u32,
    command_green_passed: bool,
) {
    let Some(RepeatPolicy {
        until: Until::CommandGreen { command },
        ..
    }) = phase.repeat.as_ref()
    else {
        return;
    };
    state.record_command_green(&phase.id, command, rounds_run, command_green_passed);
}

#[derive(Clone, Copy)]
struct CommandGreenOutcome {
    passed: bool,
    quality_red: bool,
    /// The checker's exit when a command actually ran — the source of the
    /// objective verdict; `None` when no checker is configured.
    command_exit: Option<i32>,
}

fn command_green_outcome_for_current_tree(
    phase: &NormalizedPhase,
    opts: &RunOptions,
    selective_repeat: bool,
    base_items: &[String],
    accumulated: &[ItemResult],
) -> Option<CommandGreenOutcome> {
    let RepeatPolicy {
        until: Until::CommandGreen { command },
        ..
    } = phase.repeat.as_ref()? else {
        return None;
    };
    let check_exit = opts.check.map(|check| check(command));
    let command_is_green = check_exit == Some(0);
    let semantic_gate_passed =
        !selective_repeat || latest_results_all_semantic_pass(base_items, accumulated);
    Some(CommandGreenOutcome {
        passed: command_is_green && semantic_gate_passed,
        quality_red: check_exit.is_some_and(|exit| exit > 0),
        command_exit: check_exit,
    })
}

fn semantic_cacheable_phase(phase: &NormalizedPhase) -> bool {
    !phase.prompt.contains("{seen}")
}

fn seed_semantic_cache_passes(
    phase: &NormalizedPhase,
    input_text: &str,
    base_items: &[String],
    opts: &RunOptions,
    accumulated: &mut Vec<ItemResult>,
) {
    if !semantic_cacheable_phase(phase) {
        return;
    }
    let Some(cache) = opts.semantic_cache else {
        return;
    };
    for (index, item) in base_items.iter().enumerate() {
        let key = semantic_cache_key(phase, input_text, item);
        if let Some(receipt) = cache.load_pass(&key) {
            accumulated.push(cached_pass_item(index, item, receipt));
        }
    }
}

fn store_semantic_cache_passes(
    phase: &NormalizedPhase,
    input_text: &str,
    opts: &RunOptions,
    items: &[ItemResult],
) {
    if !semantic_cacheable_phase(phase) {
        return;
    }
    let Some(cache) = opts.semantic_cache else {
        return;
    };
    for item in items {
        if item.carried && item.agent_id == "semantic-cache" {
            continue;
        }
        let Some("pass") = semantic_verdict(&item.status, item.structured.as_ref()) else {
            continue;
        };
        if let SemanticVerdict::Pass(receipt) = normalize_semantic_verdict(
            item.index,
            &item.input,
            &item.status,
            item.structured.as_ref(),
        ) {
            cache.store_pass(&semantic_cache_key(phase, input_text, &item.input), &receipt);
        }
    }
}

fn collect_pass_receipts(items: &[ItemResult]) -> Vec<PassReceipt> {
    items
        .iter()
        .filter_map(|item| match normalize_semantic_verdict(
            item.index,
            &item.input,
            &item.status,
            item.structured.as_ref(),
        ) {
            SemanticVerdict::Pass(receipt) => Some(receipt),
            _ => None,
        })
        .collect()
}

fn cached_pass_item(index: usize, item: &str, receipt: PassReceipt) -> ItemResult {
    ItemResult {
        index,
        input: item.to_string(),
        agent_id: "semantic-cache".to_string(),
        status: STATUS_COMPLETED.to_string(),
        result: Some(receipt.coverage.clone()),
        error: None,
        structured: Some(serde_json::json!({
            "verdict": "pass",
            "coverage": receipt.coverage,
            "receipt_key": receipt.receipt_key,
        })),
        output_tokens: 0,
        loaded_skills: Vec::new(),
        semantic_verdict: Some("pass".to_string()),
        retry_key: Some(receipt.receipt_key),
        carry_reason: Some("cross-run semantic cache pass".to_string()),
        carried: false,
    }
}

fn semantic_cache_key(phase: &NormalizedPhase, input_text: &str, item: &str) -> SemanticCacheKey {
    SemanticCacheKey {
        phase_id: phase.id.clone(),
        item: item.to_string(),
        schema_fingerprint: phase
            .schema
            .as_ref()
            .map_or_else(|| "none".to_string(), std::string::ToString::to_string),
        verifier_fingerprint: verifier_fingerprint(phase, input_text, item),
    }
}

/// Cache key for a semantic pass receipt. Renders with `seen = ""` on purpose:
/// the key identifies the phase's INSTRUCTION, which does not change between
/// rounds, so a receipt stored in round 1 is still findable in round 2. That is
/// also why the relay injection is gated on a non-empty `seen` — an injection
/// visible from here would move every stored key.
pub(super) fn verifier_fingerprint(
    phase: &NormalizedPhase,
    input_text: &str,
    item: &str,
) -> String {
    let command = match phase.repeat.as_ref().map(|repeat| &repeat.until) {
        Some(Until::CommandGreen { command }) => command.as_str(),
        _ => "",
    };
    serde_json::json!({
        "prompt": render_prompt(&phase.prompt, item, 0, input_text, "", phase.schema.as_ref()),
        "subagent_type": phase.subagent_type.as_deref(),
        "command_green": command,
    })
    .to_string()
}

fn select_retry_work(
    base_items: &[String],
    accumulated: &mut [ItemResult],
    quality_failures_by_item: &HashMap<usize, u32>,
) -> RetrySelection {
    let mut selected = Vec::new();
    let mut carried = 0usize;
    let mut retried = 0usize;
    let mut skipped = 0usize;

    for (index, item) in base_items.iter().enumerate() {
        let prior_failures = quality_failures_by_item.get(&index).copied().unwrap_or(0);
        let latest = accumulated.iter_mut().rev().find(|result| result.index == index);
        match latest.and_then(|result| semantic_verdict(&result.status, result.structured.as_ref()).map(|verdict| (verdict, result))) {
            Some(("pass", result)) => {
                carried += 1;
                skipped += 1;
                result.carried = true;
                result.semantic_verdict = Some("pass".to_string());
                result.carry_reason = Some("semantic pass carried into next repeat round".to_string());
            }
            Some(("finding" | "retry", _)) => {
                retried += 1;
                selected.push(RoundWorkItem {
                    index,
                    item: item.clone(),
                    prior_failures,
                });
            }
            _ => {
                selected.push(RoundWorkItem {
                    index,
                    item: item.clone(),
                    prior_failures,
                });
            }
        }
    }

    RetrySelection { items: selected, carried, retried, skipped }
}

fn record_quality_failures(
    round_items: &[ItemResult],
    quality_failures_by_item: &mut HashMap<usize, u32>,
) {
    for item in round_items {
        // `finding` is a completed, structured quality verdict. `retry`
        // denotes malformed/missing structured output, while failed transport
        // and provider attempts have no verdict; neither can unlock premium
        // implementation models.
        if semantic_verdict(&item.status, item.structured.as_ref()) == Some("finding") {
            let failures = quality_failures_by_item.entry(item.index).or_default();
            *failures = failures.saturating_add(1);
        }
    }
}

fn record_round_outcome(
    round_items: &[ItemResult],
    selective_repeat: bool,
    quality_failures_by_item: &mut HashMap<usize, u32>,
) -> Vec<usize> {
    // Transport/provider failures are not completed and never enter either
    // quality ledger path.
    let completed_indices = round_items
        .iter()
        .filter(|item| item.status == STATUS_COMPLETED)
        .map(|item| item.index)
        .collect();
    if selective_repeat {
        record_quality_failures(round_items, quality_failures_by_item);
    }
    completed_indices
}

fn record_completed_quality_failures(
    completed_indices: &[usize],
    quality_failures_by_item: &mut HashMap<usize, u32>,
) {
    for index in completed_indices {
        let failures = quality_failures_by_item.entry(*index).or_default();
        *failures = failures.saturating_add(1);
    }
}

#[allow(clippy::too_many_arguments)] // one round's command-green decision context
fn command_green_after_round(
    phase: &NormalizedPhase,
    opts: &RunOptions,
    registry: Option<&crate::misc_tools::AgentRegistry>,
    selective_repeat: bool,
    base_items: &[String],
    accumulated: &[ItemResult],
    completed_indices: &[usize],
    quality_failures_by_item: &mut HashMap<usize, u32>,
) -> bool {
    // The command runs in the main working tree after per-batch worktrees are
    // torn down, so it gates only durable implementation state.
    let Some(outcome) = command_green_outcome_for_current_tree(
        phase,
        opts,
        selective_repeat,
        base_items,
        accumulated,
    ) else {
        return false;
    };
    // A single-item phase's command result is an OBJECTIVE verdict on that
    // item's work — the one case a whole-tree gate has a clean model to
    // credit or blame. Recorded as a verify decision with basis objective so
    // the accuracy report can tell "a command proved it" from "a model said
    // so". No checker configured → no verdict, never a fabricated one.
    if base_items.len() == 1 {
        if let Some(passed) = attribution::objective_verdict_from_exit(outcome.command_exit) {
            if let Some(item) = accumulated.iter().rev().find(|item| item.index == 0) {
                attribution::record_verdict_outcome_for_agent(
                    registry,
                    &item.agent_id,
                    passed,
                    attribution::VerdictKind::PassFail,
                    runtime::VerdictBasis::Objective,
                );
            }
        }
    }
    if outcome.quality_red
        && !selective_repeat
        && opts.check.is_some()
        && base_items.len() == 1
    {
        // A completed implementation followed by a red command is a quality
        // failure only when it can be attributed to that one work item. A
        // global red check cannot safely blame every item in a fan-out.
        record_completed_quality_failures(completed_indices, quality_failures_by_item);
    }
    outcome.passed
}

fn latest_results_all_semantic_pass(base_items: &[String], accumulated: &[ItemResult]) -> bool {
    base_items.iter().enumerate().all(|(index, _)| {
        accumulated
            .iter()
            .rev()
            .find(|result| result.index == index)
            .and_then(|result| semantic_verdict(&result.status, result.structured.as_ref()))
            == Some("pass")
    })
}

fn clamp_round_work_to_budget(items: &mut Vec<RoundWorkItem>, state: &EngineState) -> bool {
    let Some(remaining) = state.remaining() else {
        return false;
    };
    if items.len() <= remaining {
        return false;
    }
    items.truncate(remaining);
    true
}

/// One unit of work for [`spawn_and_collect`]: the rendered prompt plus the
/// identity (`index`) and `{item}` text to record on the resulting
/// [`ItemResult`].
pub(super) struct SpawnUnit {
    pub(super) index: usize,
    pub(super) item: String,
    pub(super) prompt: String,
    pub(super) prior_failures: u32,
}

/// Wait for a batch of agents and fold each one's reported output tokens into
/// the running budget. The single token-accounting chokepoint: every engine
/// wait — the fan-out batches *and* the synthesize / judge / schema-retry
/// singletons — goes through here, so the tally cannot drift between paths the
/// way a duplicated counter would. Returns the completions untouched.
pub(super) fn wait_and_account(
    backend: &mut dyn AgentBackend,
    state: &mut EngineState,
    ids: &[String],
    timeout: Duration,
) -> Vec<AgentCompletion> {
    wait_and_account_observed(backend, state, ids, timeout, None)
}

/// [`wait_and_account`] plus live per-agent progress: when a sink and phase id
/// are supplied, an [`ProgressEvent::AgentDone`] is emitted the moment each
/// agent completes — in completion order, while the barrier is still
/// collecting — so the viewer's phase tally moves before `PhaseDone`.
pub(super) fn wait_and_account_observed(
    backend: &mut dyn AgentBackend,
    state: &mut EngineState,
    ids: &[String],
    timeout: Duration,
    progress: Option<(&dyn ProgressSink, &str)>,
) -> Vec<AgentCompletion> {
    let mut completions = match progress {
        Some((sink, phase_id)) => {
            let mut on_done = |completion: &AgentCompletion| {
                sink.emit(ProgressEvent::AgentDone {
                    phase_id,
                    agent_id: &completion.agent_id,
                    status: &completion.status,
                });
            };
            backend.wait_observed(ids, timeout, &mut on_done)
        }
        None => backend.wait(ids, timeout),
    };
    let terminal_ids: HashSet<&str> = completions
        .iter()
        .filter(|completion| completion.status != STATUS_STILL_RUNNING)
        .map(|completion| completion.agent_id.as_str())
        .collect();
    let mut cancelled = Vec::new();
    let mut cancelled_ids = HashSet::new();
    for id in ids {
        if terminal_ids.contains(id.as_str()) {
            continue;
        }
        if let Some(completion) = backend.cancel(id) {
            cancelled_ids.insert(id.clone());
            if let Some((sink, phase_id)) = progress {
                sink.emit(ProgressEvent::AgentDone {
                    phase_id,
                    agent_id: &completion.agent_id,
                    status: &completion.status,
                });
            }
            cancelled.push(completion);
        }
    }
    completions.retain(|completion| {
        completion.status != STATUS_STILL_RUNNING || !cancelled_ids.contains(&completion.agent_id)
    });
    completions.extend(cancelled);
    for completion in &completions {
        state.record_usage(completion.run.output_tokens, backend.usage_cost(&completion.agent_id));
    }
    completions
}

fn collect_skill_receipts(
    backend: &dyn AgentBackend,
    ids: &[String],
) -> HashMap<String, Vec<String>> {
    ids.iter()
        .map(|id| {
            let loaded = backend
                .activity(id)
                .map_or_else(Vec::new, |activity| activity.loaded_skills);
            (id.clone(), loaded)
        })
        .collect()
}

/// Spawn the units with bounded live workers, assemble each into an [`ItemResult`],
/// then run timeout recovery and schema retries. Only enforced read-only work
/// without isolation may refill individual slots; other work keeps batch barriers.
/// The two callers differ only in how they produce the units:
/// a `phases` round enumerates a fan-out (`{seen}`-rendered); a `pipeline` stage
/// carries each chain's index and feeds the prior stage's result as `{item}`.
///
/// ## Why batch (the "hundreds of agents" case)
///
/// `backend.spawn` launches a real OS thread per agent eagerly, while the
/// provider semaphore only bounds the *API* concurrency. A phase fanning out to
/// hundreds of items would therefore create hundreds of threads at once even
/// though only `workflow_concurrency_limit()` can actually run. Spawning in
/// windows of that size keeps live OS threads bounded — the Claude Code "queue
/// and run as slots free up" model. Each batch's worktree guards are dropped
/// after its barrier (RAII), so isolation dirs are torn down promptly too.
pub(super) fn spawn_and_collect(
    phase: &NormalizedPhase,
    input_text: &str,
    units: Vec<SpawnUnit>,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
) -> Vec<ItemResult> {
    let window = crate::misc_tools::workflow_concurrency_limit().max(1);
    let refill = iso.is_none() && backend.allows_readonly_refill();
    let mut items = if refill {
        super::rolling::spawn_and_collect(phase, units, backend, opts, state, window)
    } else {
        spawn_batched(phase, units, backend, opts, state, iso, window)
    };
    if !refill || items.iter().all(|item| !backend.worker_is_live(&item.agent_id)) {
        timeout_retry_pass(phase, input_text, &mut items, backend, opts, state, iso);
        if let Some(schema) = phase.schema.as_ref() {
            schema_retry_pass(phase, input_text, schema, &mut items, backend, opts, state);
        }
    }
    items
}

pub(super) fn spawn_batched(
    phase: &NormalizedPhase,
    units: Vec<SpawnUnit>,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
    window: usize,
) -> Vec<ItemResult> {
    let mut items_out: Vec<ItemResult> = Vec::with_capacity(units.len());
    let mut units = units.into_iter();

    loop {
        let batch: Vec<SpawnUnit> = units.by_ref().take(window).collect();
        if batch.is_empty() || is_cancelled(opts.cancel) {
            break;
        }
        if state.budget_blocks_spawn() {
            state.mark_exhausted();
            break;
        }

        // Worktree guards live until this batch's barrier `wait` below: each
        // agent runs inside its own dir, torn down (RAII) once the batch is
        // collected. Failure to create one degrades that agent to no isolation
        // (counted for an honest note) rather than aborting the phase.
        let mut guards: Vec<Box<dyn WorktreeGuard>> = Vec::new();
        let mut spawned: Vec<Spawned> = Vec::with_capacity(batch.len());
        for unit in batch {
            let cwd = iso.and_then(|ctx| {
                if let Ok(guard) = ctx.provider.create(&phase.id) {
                    if let Some(warning) = guard.creation_warning() {
                        state.record_worktree_warning(warning);
                    }
                    let path = guard.path().to_path_buf();
                    guards.push(guard);
                    Some(path)
                } else {
                    state.record_worktree_fallback();
                    None
                }
            });
            let input = phase_agent_input(
                phase,
                unit.index,
                &unit.item,
                unit.prompt,
                unit.prior_failures,
                false,
                cwd,
            );
            let outcome = match backend.spawn(input) {
                Ok(id) => {
                    state.record_spawn();
                    Ok(id)
                }
                Err(error) => Err(error.to_string()),
            };
            spawned.push(Spawned {
                index: unit.index,
                item: unit.item,
                outcome,
            });
        }

        let ids: Vec<String> = spawned
            .iter()
            .filter_map(|s| s.outcome.as_ref().ok().cloned())
            .collect();
        if let Some(sink) = opts.progress {
            // Accumulates across batches: the phase's full agent set builds up.
            sink.emit(ProgressEvent::AgentsSpawned {
                phase_id: &phase.id,
                agent_ids: &ids,
            });
        }
        let completions = if ids.is_empty() {
            Vec::new()
        } else {
            wait_and_account_observed(
                backend,
                state,
                &ids,
                opts.phase_timeout,
                opts.progress.map(|sink| (sink, phase.id.as_str())),
            )
        };
        let skill_receipts = collect_skill_receipts(backend, &ids);
        items_out.extend(
            spawned
                .into_iter()
                .map(|s| {
                    let loaded = s
                        .outcome
                        .as_ref()
                        .ok()
                        .and_then(|id| skill_receipts.get(id))
                        .cloned()
                        .unwrap_or_default();
                    assemble_item(s, &completions, phase.schema.as_ref(), loaded)
                }),
        );
        // Merge each agent's change-set back into the main tree *before* the
        // guards drop (teardown removes the worktree). Only runs under
        // `apply:"sequential"`; a clean tree contributes no patch.
        if let Some(ctx) = iso.filter(|ctx| ctx.merge_back) {
            merge_back_batch(&guards, ctx.provider, &phase.id, state);
        }
        // `guards` drop here, removing this batch's worktrees before the next.
    }

    items_out
}

/// Kill switch for bounded timeout/startup recovery:
/// `ZO_WORKFLOW_TIMEOUT_RETRY=off` (or `0`) restores the old behavior —
/// a stopped agent fails its phase outright and dependent phases cancel.
/// Missing/other values leave the retry on (the grind/cascade env idiom).
fn timeout_retry_enabled() -> bool {
    !std::env::var("ZO_WORKFLOW_TIMEOUT_RETRY").is_ok_and(|value| {
        let value = value.trim();
        value.eq_ignore_ascii_case("off") || value == "0"
    })
}

/// Whether this item is a phase-timeout casualty (as opposed to a user cancel
/// or a genuine failure): stopped, carrying the barrier's exact stop marker.
fn item_stopped_at_phase_timeout(item: &ItemResult) -> bool {
    item.status == STATUS_STOPPED
        && item.error.as_deref() == Some(PHASE_TIMEOUT_STOP_ERROR)
}

fn item_stopped_at_startup_watchdog(item: &ItemResult) -> bool {
    item.status == STATUS_STOPPED
        && item.error.as_deref() == Some(super::STARTUP_NO_PROGRESS_STOP_ERROR)
}

fn item_needs_recovery(item: &ItemResult) -> bool {
    item_stopped_at_phase_timeout(item) || item_stopped_at_startup_watchdog(item)
}

/// Cap on the salvaged partial output embedded in a timeout-retry prompt. The
/// streamed `output_tail` is already length-capped upstream; this is a final
/// guard so the retry prompt can never balloon.
const TIMEOUT_RETRY_PARTIAL_MAX_CHARS: usize = 8_000;

/// Build the second-attempt prompt: the re-rendered base task (`{seen}`
/// context from the first attempt is not reproducible here; the schema retry
/// pass accepts the same limit) plus the salvage-and-finish instruction.
fn timeout_retry_prompt(phase: &NormalizedPhase, input_text: &str, item: &ItemResult) -> String {
    let base = render_prompt(
        &phase.prompt,
        &item.input,
        item.index,
        input_text,
        "",
        phase.schema.as_ref(),
    );
    let partial = item
        .result
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if item_stopped_at_startup_watchdog(item) {
        return match partial {
            Some(partial) => {
                let capped =
                    core_types::text::elide_middle(partial, TIMEOUT_RETRY_PARTIAL_MAX_CHARS);
                format!(
                    "{base}\n\n[Startup recovery] The previous attempt showed no task progress before the startup deadline. Begin with concrete file/evidence inspection and produce the deliverable directly; do not spend another turn restating the plan. Salvaged partial output (may be truncated):\n---\n{capped}\n---"
                )
            }
            None => format!(
                "{base}\n\n[Startup recovery] The previous attempt showed no task progress before the startup deadline. Begin with concrete file/evidence inspection and produce the deliverable directly; do not spend another turn restating the plan."
            ),
        };
    }
    match partial {
        Some(partial) => {
            let capped = core_types::text::elide_middle(partial, TIMEOUT_RETRY_PARTIAL_MAX_CHARS);
            format!(
                "{base}\n\n[Timeout retry] Your previous attempt was force-stopped at the phase time limit before it finished. Do not start over with broad exploration — go straight to producing the deliverable. Partial output from the stopped attempt (may be truncated):\n---\n{capped}\n---\nFinish the remaining work and return the complete result."
            )
        }
        None => format!(
            "{base}\n\n[Timeout retry] Your previous attempt was force-stopped at the phase time limit before producing any output. Produce the deliverable directly and decisively; keep exploration minimal."
        ),
    }
}

pub(super) struct AlternateProviderRoute {
    pub(super) model: String,
    pub(super) remaining_fallbacks: Vec<String>,
}

/// Pick an actual Smart-router fallback on a different provider and discard
/// every candidate from the stalled provider. Explicit phase model pins never
/// enter this path (the caller checks them), preserving user authority.
fn alternate_provider_route(
    activity: &crate::misc_tools::AgentActivitySnapshot,
) -> Option<AlternateProviderRoute> {
    let selected = activity.selected_model.as_deref()?;
    let inventory = runtime::connected_model_inventory(selected);
    alternate_provider_route_in_inventory(activity, &inventory)
}

/// Select against the same provider identities the Smart Router used. The API
/// transport kind is intentionally too coarse here: a custom `DeepSeek` endpoint
/// and first-party ChatGPT are both OpenAI-compatible, but they are independent
/// providers with separate capacity and credentials.
pub(super) fn alternate_provider_route_in_inventory(
    activity: &crate::misc_tools::AgentActivitySnapshot,
    inventory: &runtime::ModelInventory,
) -> Option<AlternateProviderRoute> {
    let selected = activity.selected_model.as_deref()?;
    let mut alternatives = activity
        .fallback_models
        .iter()
        .filter(|model| {
            model.as_str() != selected
                && !same_route_provider(inventory, selected, model.as_str())
        })
        .cloned();
    let model = alternatives.next()?;
    Some(AlternateProviderRoute {
        model,
        remaining_fallbacks: alternatives.collect(),
    })
}

fn same_route_provider(
    inventory: &runtime::ModelInventory,
    left: &str,
    right: &str,
) -> bool {
    match (inventory.find(left), inventory.find(right)) {
        (Some(left), Some(right)) => left.provider().eq_ignore_ascii_case(right.provider()),
        // A fallback normally came from this exact connected inventory. Keep a
        // conservative compatibility fallback for a stale/legacy manifest whose
        // model disappeared between attempts.
        _ => api::detect_provider_kind(left) == api::detect_provider_kind(right),
    }
}

fn startup_fallback_prompt(
    phase: &NormalizedPhase,
    input_text: &str,
    item: &ItemResult,
    stalled_model: Option<&str>,
) -> String {
    let base = render_prompt(
        &phase.prompt,
        &item.input,
        item.index,
        input_text,
        "",
        phase.schema.as_ref(),
    );
    let stalled = stalled_model.unwrap_or("the prior provider");
    format!(
        "{base}\n\n[Startup provider fallback] Two attempts on {stalled} produced no task action. That provider is excluded for this bounded final attempt. Start with concrete evidence immediately, keep exploration minimal, and return the complete deliverable."
    )
}

struct RecoveryAttempt {
    agent_id: String,
    completions: Vec<AgentCompletion>,
}

impl RecoveryAttempt {
    fn completed(&self) -> bool {
        self.completions.iter().any(|completion| {
            completion.agent_id == self.agent_id && completion.status == STATUS_COMPLETED
        })
    }

    fn error_or(&self, fallback: &str) -> String {
        self.completions
            .iter()
            .find(|completion| completion.agent_id == self.agent_id)
            .and_then(|completion| completion.error.clone())
            .unwrap_or_else(|| fallback.to_string())
    }
}

fn run_recovery_attempt(
    phase_id: &str,
    input: AgentInput,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
) -> Result<RecoveryAttempt, String> {
    let agent_id = backend.spawn(input).map_err(|error| error.to_string())?;
    state.record_spawn();
    if let Some(sink) = opts.progress {
        sink.emit(ProgressEvent::AgentsSpawned {
            phase_id,
            agent_ids: std::slice::from_ref(&agent_id),
        });
    }
    let completions = wait_and_account_observed(
        backend,
        state,
        std::slice::from_ref(&agent_id),
        opts.phase_timeout,
        opts.progress.map(|sink| (sink, phase_id)),
    );
    Ok(RecoveryAttempt {
        agent_id,
        completions,
    })
}

fn pin_startup_retry_to_selected_model(
    phase: &NormalizedPhase,
    activity: Option<&crate::misc_tools::AgentActivitySnapshot>,
    input: &mut AgentInput,
) {
    if phase.model.is_some() {
        return;
    }
    let Some(activity) = activity else {
        return;
    };
    let Some(model) = activity.selected_model.as_ref() else {
        return;
    };
    input.route_model = Some(model.clone());
    input
        .route_fallback_models
        .clone_from(&activity.fallback_models);
    input.route_reason = Some(
        "startup_no_progress; retrying the selected model once with a decisive prompt".to_string(),
    );
}

fn alternate_provider_input(
    phase: &NormalizedPhase,
    input_text: &str,
    item: &ItemResult,
    cwd: Option<std::path::PathBuf>,
    activity: &crate::misc_tools::AgentActivitySnapshot,
) -> Option<(AgentInput, String)> {
    let alternate = alternate_provider_route(activity)?;
    let mut input = phase_agent_input(
        phase,
        item.index,
        &item.input,
        startup_fallback_prompt(
            phase,
            input_text,
            item,
            activity.selected_model.as_deref(),
        ),
        2,
        false,
        cwd,
    );
    input.route_model = Some(alternate.model.clone());
    input.route_fallback_models = alternate.remaining_fallbacks;
    input.route_source = Some("fallback".to_string());
    input.route_reason = Some(format!(
        "startup_no_progress twice; excluded stalled provider and selected {}",
        alternate.model
    ));
    Some((input, alternate.model))
}

fn settle_recovery_item(
    item: &mut ItemResult,
    phase: &NormalizedPhase,
    backend: &dyn AgentBackend,
    attempt: &RecoveryAttempt,
    first_stop_error: &str,
    final_error: &str,
    state: &mut EngineState,
) {
    let recovered = attempt.completed();
    state.record_timeout_retry(recovered);
    if recovered {
        let loaded_skills = backend
            .activity(&attempt.agent_id)
            .map_or_else(Vec::new, |activity| activity.loaded_skills);
        *item = assemble_item(
            Spawned {
                index: item.index,
                item: item.input.clone(),
                outcome: Ok(attempt.agent_id.clone()),
            },
            &attempt.completions,
            phase.schema.as_ref(),
            loaded_skills,
        );
        return;
    }

    // Keep the honest `stopped` status and preserve the better partial.
    item.error = Some(format!(
        "{first_stop_error}; automatic retry also failed: {final_error}"
    ));
    if item.result.as_deref().is_none_or(|text| text.trim().is_empty()) {
        if let Some(retry_partial) = attempt
            .completions
            .iter()
            .find(|completion| completion.agent_id == attempt.agent_id)
            .and_then(|completion| completion.result.clone())
        {
            item.result = Some(retry_partial);
        }
    }
}

/// Bounded recovery for phase timeout and startup-no-progress stops. Every item
/// gets one decisive retry (`prior_failures: 1`). Only when both the original
/// attempt and that retry are classified `startup_no_progress`, and the phase
/// was not explicitly model-pinned, may a final `prior_failures: 2` attempt use
/// a precomputed Smart fallback from a different provider. There is no loop.
#[allow(clippy::too_many_arguments)]
pub(super) fn timeout_retry_pass(
    phase: &NormalizedPhase,
    input_text: &str,
    items: &mut [ItemResult],
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
) {
    if !items.iter().any(item_needs_recovery) || !timeout_retry_enabled() {
        return;
    }
    for item in items.iter_mut() {
        if !item_needs_recovery(item) {
            continue;
        }
        if is_cancelled(opts.cancel) {
            break;
        }
        if state.budget_blocks_spawn() {
            state.mark_exhausted();
            break;
        }
        // The retry does real file work, so it keeps the phase's isolation
        // contract: a fresh worktree when the run is isolated, merged back
        // after the barrier under `apply:"sequential"` — exactly like a
        // first-attempt batch member.
        let mut guards: Vec<Box<dyn WorktreeGuard>> = Vec::new();
        let cwd = iso.and_then(|ctx| {
            if let Ok(guard) = ctx.provider.create(&phase.id) {
                if let Some(warning) = guard.creation_warning() {
                    state.record_worktree_warning(warning);
                }
                let path = guard.path().to_path_buf();
                guards.push(guard);
                Some(path)
            } else {
                state.record_worktree_fallback();
                None
            }
        });
        let first_stop_error = item
            .error
            .clone()
            .unwrap_or_else(|| PHASE_TIMEOUT_STOP_ERROR.to_string());
        let initial_activity = item_stopped_at_startup_watchdog(item)
            .then(|| backend.activity(&item.agent_id))
            .flatten();
        let prompt = timeout_retry_prompt(phase, input_text, item);
        let mut retry_input = phase_agent_input(
            phase,
            item.index,
            &item.input,
            prompt,
            1,
            false,
            cwd.clone(),
        );
        // Startup recovery first retries the exact selected model with a more
        // decisive prompt. That makes the second classification meaningful:
        // only two stalls on the same backend authorize provider exclusion.
        pin_startup_retry_to_selected_model(phase, initial_activity.as_ref(), &mut retry_input);
        let Ok(mut final_attempt) =
            run_recovery_attempt(&phase.id, retry_input, backend, opts, state)
        else {
            continue;
        };
        let retry_error = final_attempt.error_or("retry never produced a completion");
        let mut final_error = retry_error.clone();

        let two_startup_stalls = first_stop_error == super::STARTUP_NO_PROGRESS_STOP_ERROR
            && retry_error == super::STARTUP_NO_PROGRESS_STOP_ERROR;
        if !final_attempt.completed()
            && two_startup_stalls
            && phase.model.is_none()
            && !state.budget_blocks_spawn()
        {
            if let Some(activity) = backend.activity(&final_attempt.agent_id) {
                if let Some((fallback_input, fallback_model)) =
                    alternate_provider_input(phase, input_text, item, cwd, &activity)
                {
                    match run_recovery_attempt(&phase.id, fallback_input, backend, opts, state) {
                        Ok(attempt) => {
                            final_error = attempt.error_or(
                                "alternate-provider recovery never produced a completion",
                            );
                            final_attempt = attempt;
                        }
                        Err(error) => {
                            final_error = format!(
                                "alternate-provider recovery failed to spawn {fallback_model}: {error}"
                            );
                        }
                    }
                }
            }
        } else if !final_attempt.completed()
            && two_startup_stalls
            && state.budget_blocks_spawn()
        {
            state.mark_exhausted();
        }

        settle_recovery_item(
            item,
            phase,
            backend,
            &final_attempt,
            &first_stop_error,
            &final_error,
            state,
        );
        if let Some(ctx) = iso.filter(|ctx| ctx.merge_back) {
            merge_back_batch(&guards, ctx.provider, &phase.id, state);
        }
    }
}

/// After a batch's barrier, merge each isolated agent's changes back into the
/// main working tree in spawn order (`apply:"sequential"`). A clean worktree
/// yields no patch; a patch that fails to collect or apply is recorded as an
/// honest note (and left for manual resolution) rather than aborting the run —
/// sibling change-sets still merge.
pub(super) fn merge_back_batch(
    guards: &[Box<dyn WorktreeGuard>],
    provider: &dyn WorktreeProvider,
    phase_id: &str,
    state: &mut EngineState,
) {
    for guard in guards {
        match guard.collect_patch() {
            Ok(None) => {}
            Ok(Some(patch)) => match provider.apply_patch(&patch) {
                Ok(()) => state.record_patch_applied(),
                Err(err) => state.record_apply_failure(format!("phase `{phase_id}`: {err}")),
            },
            Err(err) => state.record_apply_failure(format!(
                "phase `{phase_id}`: collecting changes failed: {err}"
            )),
        }
    }
}

/// One fan-out round: render a `{seen}`-aware prompt for each item, then run the
/// batch through [`spawn_and_collect`]. `seen` is the accumulated prior-round
/// text substituted for `{seen}`.
#[allow(clippy::too_many_arguments)] // orchestration: phase + ambient run deps
pub(super) fn run_round(
    phase: &NormalizedPhase,
    input_text: &str,
    items: &[RoundWorkItem],
    seen: &str,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
    iso: Option<IsolationCtx>,
) -> Vec<ItemResult> {
    let units = items
        .iter()
        .map(|item| SpawnUnit {
            index: item.index,
            item: item.item.clone(),
            prior_failures: item.prior_failures,
            prompt: render_prompt(
                &phase.prompt,
                &item.item,
                item.index,
                input_text,
                seen,
                phase.schema.as_ref(),
            ),
        })
        .collect();
    spawn_and_collect(phase, input_text, units, backend, opts, state, iso)
}
