//! Synthesis and judge phases: candidate collection, judging, and
//! outcome notes.
use serde_json::Value;

use crate::misc_tools::AgentInput;

use super::super::spec::{Judge, Synthesize};
use super::items::item_text_for_mapping;
use super::prompts::{extract_structured, schema_instruction, truncate_on_char_boundary};
use super::spawn::wait_and_account;
use super::{
    AgentBackend, EngineState, FindingState, Judgement, PhaseReport, RunOptions, STATUS_COMPLETED,
    STATUS_FAILED, STATUS_STILL_RUNNING, STATUS_STOPPED, SYNTH_ITEM_CAP_BYTES,
};

/// Run the optional synthesis agent over every completed item's text.
pub(super) fn run_synthesize(
    synth: &Synthesize,
    phases: &[PhaseReport],
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
) -> Option<String> {
    let all = collect_for_synthesis(phases);
    let prompt = synth.prompt.replace("{all}", &all);
    let agent_input = AgentInput {
        route_probe_confidence: None,
        fork_source: None,
        allow_cross_provider: synth.model.is_some(),
        description: "workflow synthesize".to_string(),
        prompt,
        subagent_type: synth.subagent_type.clone(),
        name: None,
        addressable_name: None,
        model: synth.model.clone(),
        cwd: None,
        schema: None,
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
    };
    let agent_id = backend.spawn(agent_input).ok()?;
    state.record_spawn();
    let completions = wait_and_account(
        backend,
        state,
        std::slice::from_ref(&agent_id),
        opts.phase_timeout,
    );
    completions
        .into_iter()
        .find(|c| c.agent_id == agent_id && c.status == STATUS_COMPLETED)
        .and_then(|c| c.result)
}

/// Concatenate every completed item's text, per-item capped, for `{all}`.
pub(super) fn collect_for_synthesis(phases: &[PhaseReport]) -> String {
    let mut sections = Vec::new();
    for phase in phases {
        for item in &phase.items {
            if item.status != STATUS_COMPLETED {
                continue;
            }
            let text = item_text_for_mapping(item);
            if text.trim().is_empty() {
                continue;
            }
            sections.push(format!(
                "## {} [item {}]\n{}",
                phase.id,
                item.index,
                truncate_on_char_boundary(&text, SYNTH_ITEM_CAP_BYTES)
            ));
        }
        // A repair-loop phase's verdict lives in its findings, not its item text:
        // every agent can complete while the gate stays red (a blocked repair or
        // a failing `final_check`). Without these the synthesizer cannot see the
        // outcome it is asked to report.
        for finding in &phase.findings {
            sections.push(format!(
                "## {} [finding:{}]\n{}\n{}",
                phase.id,
                finding_state_label(&finding.state),
                finding.title,
                truncate_on_char_boundary(&finding.evidence, SYNTH_ITEM_CAP_BYTES)
            ));
        }
    }
    sections.join("\n\n")
}

/// A finding state's label, matching its serde `snake_case` spelling.
fn finding_state_label(state: &FindingState) -> &'static str {
    match state {
        FindingState::Queued => "queued",
        FindingState::Fixing => "fixing",
        FindingState::Verifying => "verifying",
        FindingState::Fixed => "fixed",
        FindingState::Blocked => "blocked",
        FindingState::Escalated => "escalated",
    }
}

/// JSON schema steering the judge agent's `StructuredOutput` call.
pub(super) fn judge_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "winner_index": { "type": "integer", "minimum": 0 },
            "rationale": { "type": "string" },
            "ranking": { "type": "array", "items": { "type": "integer" } }
        },
        "required": ["winner_index"]
    })
}

/// Concatenate the completed items as 0-based numbered candidate blocks, so a
/// judge's `winner_index` maps directly onto an ordinal. Returns the rendered
/// text and the candidate count (for the in-range check).
pub(super) fn collect_candidates(phases: &[PhaseReport]) -> (String, usize) {
    let mut sections = Vec::new();
    for phase in phases {
        for item in &phase.items {
            if item.status != STATUS_COMPLETED {
                continue;
            }
            let text = item_text_for_mapping(item);
            if text.trim().is_empty() {
                continue;
            }
            let n = sections.len();
            sections.push(format!(
                "### Candidate {n}\n{}",
                truncate_on_char_boundary(&text, SYNTH_ITEM_CAP_BYTES)
            ));
        }
    }
    let count = sections.len();
    (sections.join("\n\n"), count)
}

/// Run the optional judge agent: render `{candidates}`, force the
/// `StructuredOutput` schema, and parse the verdict. Mirrors
/// [`run_synthesize`] but selects a winner instead of merging. Returns `None`
/// when there is nothing to judge, the spawn fails, or the verdict is missing
/// / out of range.
pub(super) fn run_judge(
    judge: &Judge,
    phases: &[PhaseReport],
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
) -> Option<Judgement> {
    let (candidates, count) = collect_candidates(phases);
    if count == 0 {
        return None;
    }
    let schema = judge_schema();
    let prompt = format!(
        "{}{}",
        judge.prompt.replace("{candidates}", &candidates),
        schema_instruction(&schema)
    );
    let agent_input = AgentInput {
        route_probe_confidence: None,
        fork_source: None,
        allow_cross_provider: judge.model.is_some(),
        description: "workflow judge".to_string(),
        prompt,
        subagent_type: judge.subagent_type.clone(),
        name: None,
        addressable_name: None,
        model: judge.model.clone(),
        cwd: None,
        schema: Some(schema),
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
    };
    let agent_id = backend.spawn(agent_input).ok()?;
    state.record_spawn();
    let completions = wait_and_account(
        backend,
        state,
        std::slice::from_ref(&agent_id),
        opts.phase_timeout,
    );
    let completion = completions
        .into_iter()
        .find(|c| c.agent_id == agent_id && c.status == STATUS_COMPLETED)?;
    // Prefer the exact captured `StructuredOutput` (8c); fall back to parsing
    // JSON out of the free-text result.
    let value = completion
        .structured
        .or_else(|| completion.result.as_deref().and_then(extract_structured))?;
    parse_judgement(&value, count)
}

/// Parse a judge verdict, rejecting a missing or out-of-range `winner_index`
/// (a hallucinated ordinal yields no verdict rather than a bogus winner).
pub(super) fn parse_judgement(value: &Value, candidate_count: usize) -> Option<Judgement> {
    let obj = value.as_object()?;
    let winner_index = usize::try_from(obj.get("winner_index")?.as_u64()?).ok()?;
    if winner_index >= candidate_count {
        return None;
    }
    let rationale = obj
        .get("rationale")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let ranking = obj
        .get("ranking")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_u64().and_then(|n| usize::try_from(n).ok()))
                .collect()
        })
        .unwrap_or_default();
    Some(Judgement {
        winner_index,
        rationale,
        ranking,
    })
}

/// Append honest summary notes for failed / stopped / still-running agents.
pub(super) fn append_outcome_notes(reports: &[PhaseReport], notes: &mut Vec<String>) {
    let mut failed = 0usize;
    let mut stopped = 0usize;
    let mut still_running = 0usize;
    for item in reports.iter().flat_map(|phase| &phase.items) {
        match item.status.as_str() {
            STATUS_FAILED => failed += 1,
            STATUS_STOPPED => stopped += 1,
            STATUS_STILL_RUNNING => still_running += 1,
            _ => {}
        }
    }
    if failed > 0 {
        notes.push(format!(
            "{failed} agent(s) failed; their phase peers and later phases still ran"
        ));
    }
    if stopped > 0 {
        notes.push(format!(
            "{stopped} agent(s) did not finish within the phase timeout — cancel signal sent and item marked stopped"
        ));
    }
    if still_running > 0 {
        notes.push(format!(
            "{still_running} agent(s) did not finish within the phase timeout — marked still_running (backend could not stop them; results not captured)"
        ));
    }
}

// ---------------------------------------------------------------------------
// Prompt rendering + JSON extraction
// ---------------------------------------------------------------------------
