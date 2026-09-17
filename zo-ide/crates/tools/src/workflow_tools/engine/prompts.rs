//! Prompt rendering and structured-output extraction helpers.

use serde_json::Value;

use crate::misc_tools::AgentInput;

use super::super::spec::{NormalizedPhase, RepairStep};

/// Substitute the workflow tokens into a phase/synthesize prompt and, when a
/// schema is present, append the JSON-only instruction.
pub(super) fn render_prompt(
    template: &str,
    item: &str,
    index: usize,
    input_text: &str,
    seen: &str,
    schema: Option<&Value>,
) -> String {
    let mut rendered = template
        .replace("{item}", item)
        .replace("{index}", &index.to_string())
        .replace("{input}", input_text)
        // `{seen}` = accumulated prior-round results for a repeating phase
        // (empty for a non-repeat phase / the first round).
        .replace("{seen}", seen);
    // A repeat whose author never named `{seen}` still gets the relay: without
    // it round N+1 re-runs a byte-identical instruction and re-derives what
    // round N established. Measured on real runs, those phases spent 40% of
    // their output tokens after round 1 against 26% for relay-bearing ones, and
    // re-reported an already-reported file 3.0 times per round-pair against
    // 0.33 — in the worst case a round that restated an already-green verify.
    //
    // The relay is appended, not substituted, so the author's own prompt is
    // untouched and a phase that named `{seen}` never gets a second copy. It
    // costs nothing until there is something to relay: `seen` is empty on round
    // 1 and for every non-repeat phase, which keeps those prompts byte-identical
    // — including `verifier_fingerprint`, so no cache key moves.
    if !seen.is_empty() && !template.contains("{seen}") {
        rendered.push_str(&relay_instruction(seen));
    }
    if let Some(schema) = schema {
        rendered.push_str(&schema_instruction(schema));
    }
    rendered
}

/// The engine-owned relay block appended to a repeat prompt that did not name
/// `{seen}`.
///
/// It carries the INSTRUCTION, not just the data. The relay-bearing phases this
/// was measured against were preset-authored, and their prompts also say "do
/// not churn code" — so handing a bare list to a prompt that lacks that
/// sentence would relay the evidence while dropping the behavior it produced.
/// Saying it here means every phase gets both, and the wording lives beside the
/// relay rather than in a tool description an author may not have read.
fn relay_instruction(seen: &str) -> String {
    format!(
        "\n\nResults already established by earlier rounds of this phase are below. \
         Do not re-derive, re-verify, or re-apply them; treat them as done and report only \
         what is new or what changed. If they already satisfy the task, say so and stop \
         rather than restating them.\n\n{seen}"
    )
}

pub(super) fn schema_instruction(schema: &Value) -> String {
    let schema_text = serde_json::to_string(schema).unwrap_or_else(|_| schema.to_string());
    // 8c: steer the agent to the `StructuredOutput` tool (captured exactly),
    // while still accepting a bare-JSON reply as the parse-from-text fallback.
    format!(
        "\n\nReturn your result by calling the `StructuredOutput` tool exactly once with a JSON value matching this schema. If you cannot call a tool, reply with ONLY that JSON value — no prose, no code fences:\n{schema_text}"
    )
}

pub(super) fn render_fixer_prompt(
    step: &RepairStep,
    finding_id: &str,
    title: &str,
    affected_paths: &[String],
    evidence: &str,
    pass_receipts: &str,
) -> String {
    step.prompt
        .replace("{finding_id}", finding_id)
        .replace("{finding}", title)
        .replace("{affected_paths}", &affected_paths.join(", "))
        .replace("{evidence}", evidence)
        .replace("{pass_receipts}", pass_receipts)
}

pub(super) fn render_reverify_prompt(
    template: &str,
    finding_id: &str,
    title: &str,
    evidence: &str,
    invalidation_reason: &str,
    schema: Option<&Value>,
) -> String {
    let mut prompt = template
        .replace("{finding_id}", finding_id)
        .replace("{finding}", title)
        .replace("{evidence}", evidence)
        .replace("{invalidation_reason}", invalidation_reason);
    if let Some(schema) = schema {
        prompt.push_str(&schema_instruction(schema));
    }
    prompt
}

/// Extract a JSON object/array from an agent's free-text result: try a direct
/// parse first, then fall back to the first balanced `{...}` block.
pub(super) fn extract_structured(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        if value.is_object() || value.is_array() {
            return Some(value);
        }
    }
    let block = first_balanced_object(text)?;
    serde_json::from_str::<Value>(&block).ok()
}

/// Return the first balanced `{...}` substring, respecting string literals and
/// escapes. Scans bytes; the JSON delimiters are ASCII so multi-byte UTF-8
/// content never produces a false `{`/`}`/`"` and every slice lands on a
/// char boundary.
pub(super) fn first_balanced_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in text.bytes().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn value_to_prompt_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub(super) fn truncate_on_char_boundary(text: &str, max_bytes: usize) -> String {
    core_types::text::truncate_on_char_boundary(text, max_bytes, "…")
}

pub(super) fn phase_agent_input(
    phase: &NormalizedPhase,
    index: usize,
    item: &str,
    mut prompt: String,
    prior_failures: u32,
    retry: bool,
    cwd: Option<std::path::PathBuf>,
) -> AgentInput {
    let suffix = if retry { " (schema retry)" } else { "" };
    if prior_failures == 0 && !retry {
        prompt.push_str(
            "\n\n[Workflow execution] Analysis supplied in this prompt is context. Start by checking the current files or other task evidence and begin the delegated work promptly. If that evidence invalidates the plan, make the smallest necessary local correction before continuing.",
        );
    }
    AgentInput {
        route_probe_confidence: None,
        fork_source: None,
        allow_cross_provider: phase.model.is_some(),
        description: format!("workflow phase `{}` item {index}{suffix}", phase.id),
        prompt,
        subagent_type: phase.subagent_type.clone(),
        // Workflow-tree grouping key. Slugified downstream
        // (`slugify_agent_name`), so `"read:engine.rs"` becomes `read-engine-rs`
        // and the TUI can map each floating manifest back to its phase. Without
        // this the engine spawned with `name: None`, leaving no key to group by.
        name: Some(agent_display_name(&phase.id, index, item)),
        addressable_name: None,
        model: phase.model.clone(),
        cwd,
        // 8c: enable `StructuredOutput` for the agent when this phase declared a
        // schema, so its result can be captured from the tool call's input.
        schema: phase.schema.clone(),
        // Workflow agents use the higher workflow concurrency cap.
        workflow_member: true,
        one_shot: false,
        plan_shape: None,
        route_tax: None,
        // Workflow members are collected by the engine, never detached.
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
        prior_failures,
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

/// The agent's display name = its workflow-tree leaf label. A short, single-line
/// item becomes the leaf (`read:engine.rs`) so the tree reads meaningfully;
/// otherwise the index keeps it stable and bounded (`read#7`). Slugified
/// downstream, so punctuation is normalized to `-` and the length is capped.
pub(super) fn agent_display_name(phase_id: &str, index: usize, item: &str) -> String {
    let item = item.trim();
    if !item.is_empty() && !item.contains('\n') && item.chars().count() <= 24 {
        format!("{phase_id}:{item}")
    } else {
        format!("{phase_id}#{index}")
    }
}
