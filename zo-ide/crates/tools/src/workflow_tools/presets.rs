//! Built-in workflow presets.
//!
//! Presets are only an input-expansion layer: they turn a small, named request
//! into an ordinary [`WorkflowSpec`](super::spec::WorkflowSpec) JSON value. The
//! engine still owns execution, routing, retries, and verification.

use serde_json::{json, Value};

use crate::ToolError;

const CROSS_MODEL_VERIFIED: &str = "cross_model_verified";
const GPT_CLAUDE_VERIFIED: &str = "gpt_claude_verified";
const DEFAULT_VERIFY_COMMAND: &str = "cargo check --workspace --all-targets";
const DEFAULT_MAX_ROUNDS: u32 = 3;

#[derive(Debug)]
pub(super) struct PresetExpansion {
    pub spec: Value,
    pub input: Value,
}

pub(super) fn expand_preset(input: &Value) -> Result<Option<PresetExpansion>, ToolError> {
    let Some(preset) = input.get("preset") else {
        return Ok(None);
    };
    if input.get("spec").is_some() {
        return Err(invalid("`preset` and `spec` are mutually exclusive"));
    }

    let preset = preset
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("`preset` must be a non-empty string"))?;

    match preset {
        CROSS_MODEL_VERIFIED | GPT_CLAUDE_VERIFIED => Ok(Some(cross_model_verified(input)?)),
        other => Err(invalid(format!(
            "unknown workflow preset `{other}` (expected `{CROSS_MODEL_VERIFIED}` or `{GPT_CLAUDE_VERIFIED}`)"
        ))),
    }
}

fn cross_model_verified(input: &Value) -> Result<PresetExpansion, ToolError> {
    let forced = forced_agent_model_override();
    cross_model_verified_with_override(input, forced.as_deref())
}

fn cross_model_verified_with_override(
    input: &Value,
    forced_agent_model: Option<&str>,
) -> Result<PresetExpansion, ToolError> {
    let request = parse_preset_request(input, forced_agent_model)?;
    let spec = cross_model_spec(&request);
    Ok(PresetExpansion {
        spec,
        input: request.task,
    })
}

/// A validated `cross_model_verified` request: the task, the verification
/// command, the repair budget, and the resolved model lane per role.
struct PresetRequest {
    task: Value,
    verify_command: String,
    max_rounds: u32,
    design: bool,
    design_model: String,
    coding_model: String,
    review_model: String,
    repair_model: String,
    verification_model: String,
    synthesis_model: String,
}

fn parse_preset_request(
    input: &Value,
    forced_agent_model: Option<&str>,
) -> Result<PresetRequest, ToolError> {
    if let Some(forced) = forced_agent_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Err(invalid(format!(
            "`ZO_AGENT_MODEL` is set to `{forced}`, which forces every sub-agent onto one model and disables the `cross_model_verified` contract; unset it or use an explicit workflow spec"
        )));
    }

    let task = input.get("input").cloned().unwrap_or(Value::Null);
    if task_is_empty(&task) {
        return Err(invalid(
            "`input` is required for `cross_model_verified` and must not be empty",
        ));
    }

    let verify_command = match string_field(input, "verify_command")? {
        Some(command) => command,
        None => string_field(input, "verification_command")?
            .unwrap_or_else(|| DEFAULT_VERIFY_COMMAND.to_string()),
    };
    if verify_command.trim().is_empty() {
        return Err(invalid("`verify_command` must not be empty"));
    }

    let coding_model = required_string_field(input, "coding_model")?;
    let review_model = required_string_field(input, "review_model")?;
    if coding_model == review_model {
        return Err(invalid(
            "`coding_model` and `review_model` must be different for `cross_model_verified`",
        ));
    }
    let design_model =
        optional_model(input, "design_model")?.unwrap_or_else(|| review_model.clone());
    let repair_model =
        optional_model(input, "repair_model")?.unwrap_or_else(|| coding_model.clone());
    if repair_model == review_model {
        return Err(invalid(
            "`repair_model` and `review_model` must be different for `cross_model_verified`",
        ));
    }
    let verification_model =
        optional_model(input, "verification_model")?.unwrap_or_else(|| review_model.clone());
    let synthesis_model =
        optional_model(input, "synthesis_model")?.unwrap_or_else(|| review_model.clone());

    let max_rounds = u32_field(input, "max_rounds")?.unwrap_or(DEFAULT_MAX_ROUNDS);
    if max_rounds == 0 {
        return Err(invalid("`max_rounds` must be at least 1"));
    }

    Ok(PresetRequest {
        task,
        verify_command,
        max_rounds,
        design: bool_field(input, "design")?.unwrap_or(false),
        design_model,
        coding_model,
        review_model,
        repair_model,
        verification_model,
        synthesis_model,
    })
}

/// An optional model pin, treating blank as absent: a bare `""` would otherwise
/// survive as an empty phase pin that `spec::clean_opt` silently drops, quietly
/// detaching that phase from the preset's model contract.
fn optional_model(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    Ok(string_field(input, key)?.filter(|value| !value.is_empty()))
}

/// The preflight phase's `required` list and its design clause. `design: true`
/// promotes the UI brief from an "if it looks like UI" hedge to a required part
/// of the contract, so the implementer always receives design tokens and
/// acceptance criteria to build against.
fn preflight_design_contract(design: bool) -> (Value, String) {
    let (required, clause) = if design {
        (
            json!(["can_avoid_new_code", "minimal_approach", "design_plan", "acceptance_criteria"]),
            "This task is user-facing UI/UX, so `design_plan` (audience, layout, design tokens — color/type/spacing — and the one signature element) and `acceptance_criteria` (concrete interaction, responsive, and accessibility checks the implementation must satisfy) are REQUIRED, not optional.",
        )
    } else {
        (
            json!(["can_avoid_new_code", "minimal_approach"]),
            "If the task is user-facing UI/UX, also fill `design_plan` (audience, layout, design tokens — color/type/spacing — and the one signature element) and `acceptance_criteria` (concrete interaction, responsive, and accessibility checks the implementation must satisfy); omit both for non-UI tasks.",
        )
    };
    (
        required,
        format!(
            "Preflight the task before any implementation. Decide whether existing code, standard library APIs, existing dependencies, configuration, or documentation can solve it with less new code. Be conservative: do not invent APIs. {clause} Return the required JSON only. Task:\n{{input}}"
        ),
    )
}

/// The structured verdict shape the engine's repair loop classifies
/// (`engine::items::normalize_semantic_verdict`): `fail` + evidence becomes an
/// actionable finding for the fixer lane; `pass` needs `coverage` to count as a
/// receipt.
pub(crate) fn verdict_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": { "type": "string", "enum": ["pass", "fail"] },
            "coverage": { "type": "string" },
            "title": { "type": "string" },
            "evidence": { "type": "string" },
            "affected_paths": { "type": "array", "items": { "type": "string" } },
            "risk": { "type": "string", "enum": ["local", "shared", "global"] }
        },
        "required": ["verdict"]
    })
}

/// Render the preset's five-phase spec. Kept separate from
/// [`parse_preset_request`] so validation and rendering stay independently
/// readable.
fn cross_model_spec(request: &PresetRequest) -> Value {
    let (preflight_required, preflight_prompt) = preflight_design_contract(request.design);
    let verdict_schema = verdict_schema();
    let PresetRequest {
        design_model, coding_model, review_model, repair_model,
        verification_model, synthesis_model, verify_command, max_rounds, ..
    } = request;
    json!({
            "name": "cross-model-verified",
            "description": "Built-in preset: preflight minimality + design brief, coding-role implementation, reviewer-role critique, coding-role repair, and a verifier-role final audit that is a fix_until_verified hard gate on the verification command.",
            "mode": "phases",
            "phases": [
                {
                    "id": "preflight",
                    "subagent_type": "Plan",
                    "model": design_model,
                    "schema": {
                        "type": "object",
                        "properties": {
                            "can_avoid_new_code": { "type": "boolean" },
                            "minimal_approach": { "type": "string" },
                            "files_to_inspect": { "type": "array", "items": { "type": "string" } },
                            "risks": { "type": "array", "items": { "type": "string" } },
                            "design_plan": { "type": "string" },
                            "acceptance_criteria": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": preflight_required
                    },
                    "prompt": preflight_prompt
                },
                {
                    "id": "implement",
                    "over": "preflight",
                    "subagent_type": "general-purpose",
                    "model": coding_model,
                    "prompt": "Implement the task using the preflight result below. Prefer the smallest correct change, clean code, single responsibility, no speculative abstractions, and no unrelated cleanup. If preflight says new code can be avoided, do that instead of generating needless code. Do not commit. For library or framework APIs, fetch current documentation through a connected docs MCP tool (such as `context7`) instead of relying on training memory; for UI/frontend work, load the `frontend-design` skill before writing code, follow the preflight `design_plan`, and satisfy every `acceptance_criteria` entry. When the preflight result carries a `design_plan` or `acceptance_criteria`, restate them verbatim in your final report under a `Design contract` heading — your report is the only thing the reviewer receives.\n\nTask:\n{input}\n\nPreflight result:\n{item}"
                },
                {
                    "id": "adversarial_review",
                    "over": "implement",
                    "subagent_type": "code-reviewer",
                    "model": review_model,
                    "prompt": "Adversarially review the current working tree after the implementation. Assume a different model wrote it. Check correctness, security, concurrency, error handling, edge cases, tests, literal/spec mismatches, SRP, and unnecessary complexity. For UI-facing work, also check the result against the `Design contract` section of the implementation handoff below and every `acceptance_criteria` entry it lists (interaction, responsive, accessibility); if a UI-facing handoff carries no `Design contract` section, report that absence itself as a blocker. Report only actionable blockers or high-value fixes; if clean, say so clearly.\n\nTask:\n{input}\n\nImplementation handoff:\n{item}"
                },
                {
                    "id": "repair_until_green",
                    "over": "adversarial_review",
                    "subagent_type": "general-purpose",
                    "model": repair_model,
                    "prompt": "Apply only valid fixes from the review, then ensure the verification command is likely to pass. Keep changes surgical and SRP-friendly. If the review is already clean, do not churn code; explain the no-op. For library or framework APIs, fetch current documentation through a connected docs MCP tool (such as `context7`) instead of relying on training memory; for UI/frontend work, load the `frontend-design` skill before writing code. Previous repair rounds are below.\n\nTask:\n{input}\n\nReview:\n{item}\n\nPrior repair rounds:\n{seen}",
                    "repeat": {
                        "max_rounds": max_rounds,
                        "until": { "command_green": { "command": verify_command } }
                    }
                },
                {
                    "id": "final_verification",
                    "over": "repair_until_green",
                    "subagent_type": "Verification",
                    "model": verification_model,
                    "schema": verdict_schema,
                    "prompt": "Final verification pass from an independent verifier role. Inspect the current diff and relevant tests. Confirm whether the task is complete, the verification command is appropriate, and no avoidable complexity or SRP violation remains. Do not edit files. For UI-facing changes, require visual verification evidence from a real run or render, not just passing checks. Return `verdict` \"pass\" with `coverage` naming exactly what you checked, or \"fail\" with `title`, `evidence`, `affected_paths`, and `risk` for the most important defect.\n\nTask:\n{input}\n\nRepair handoff:\n{item}",
                    "strategy": "fix_until_verified",
                    "validator": {
                        "prompt": "Focused reverify of finding {finding_id} ({finding}) against the current working tree after its fix. Do not edit files. Return `verdict` \"pass\" with `coverage`, or \"fail\" with `title`, `evidence`, `affected_paths`, and `risk`.",
                        "model": verification_model,
                        "schema": verdict_schema
                    },
                    "fixer": {
                        "prompt": "Fix exactly this verified finding — surgically, no unrelated cleanup, do not commit.\nFinding {finding_id}: {finding}\nEvidence:\n{evidence}\nAffected paths: {affected_paths}\nCarried pass receipts (avoid invalidating them needlessly):\n{pass_receipts}",
                        "model": repair_model
                    },
                    "final_check": { "command": verify_command },
                    "max_attempts": max_rounds
                }
            ],
            "synthesize": {
                "subagent_type": "Plan",
                "model": synthesis_model,
                "prompt": "Summarize the cross-model workflow result for the parent agent. Include: final status, decisive verification evidence, remaining risks, and whether commit/push is safe. If any phase reports blocked findings or a verification command that did not pass, the final status MUST be FAILED and commit/push unsafe. Use only the completed workflow outputs below.\n\n{all}"
            }
    })
}

fn required_string_field(input: &Value, key: &str) -> Result<String, ToolError> {
    string_field(input, key)?.ok_or_else(|| invalid(format!("`{key}` is required")))
}

fn string_field(input: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.trim().to_string())),
        Some(_) => Err(invalid(format!("`{key}` must be a string"))),
    }
}

fn u32_field(input: &Value, key: &str) -> Result<Option<u32>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .map(Some)
            .ok_or_else(|| invalid(format!("`{key}` must be a positive integer"))),
        Some(_) => Err(invalid(format!("`{key}` must be a positive integer"))),
    }
}

/// Strict boolean read: a truthy string or number is a caller mistake, not a
/// silent `true` — `design` sits next to the string field `design_model`, so a
/// misplaced value must fail loudly instead of changing the preflight contract.
fn bool_field(input: &Value, key: &str) -> Result<Option<bool>, ToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(invalid(format!("`{key}` must be a boolean"))),
    }
}

fn task_is_empty(task: &Value) -> bool {
    match task {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        _ => false,
    }
}

#[cfg(not(test))]
fn forced_agent_model_override() -> Option<String> {
    std::env::var("ZO_AGENT_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
fn forced_agent_model_override() -> Option<String> {
    None
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::InvalidInput(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_cross_model_verified_preset() {
        let expanded = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "verify_command": "cargo test -p tools",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "verification_model": "claude-sonnet-verify",
            "synthesis_model": "gpt-synth",
            "max_rounds": 2
        }))
        .expect("preset should parse")
        .expect("preset should expand");

        assert_eq!(expanded.input, json!("fix the parser"));
        assert_eq!(expanded.spec["name"], "cross-model-verified");
        assert_eq!(expanded.spec["phases"].as_array().expect("phases").len(), 5);
        assert_eq!(
            expanded.spec["phases"][1]["subagent_type"],
            "general-purpose",
            "implementation must route through the coding role"
        );
        assert_eq!(
            expanded.spec["phases"][2]["subagent_type"],
            "code-reviewer",
            "review must route through an independent reviewer role"
        );
        assert_eq!(
            expanded.spec["phases"][1]["model"],
            "gpt-5.5-fast",
            "implementation must use the explicit coding model"
        );
        assert_eq!(
            expanded.spec["phases"][2]["model"],
            "claude-opus-4-8",
            "review must use the explicit review model"
        );
        assert_eq!(
            expanded.spec["phases"][4]["model"],
            "claude-sonnet-verify",
            "final verifier may use a dedicated verification model"
        );
        assert_eq!(
            expanded.spec["synthesize"]["model"],
            "gpt-synth",
            "synthesis may use a dedicated synthesis model"
        );
        assert_eq!(
            expanded.spec["phases"][3]["repeat"]["until"]["command_green"]["command"],
            "cargo test -p tools"
        );
    }

    #[test]
    fn alias_expands_like_primary_preset() {
        let primary = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8"
        }))
        .expect("primary preset should parse")
        .expect("primary preset should expand");
        let alias = expand_preset(&json!({
            "preset": "gpt_claude_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8"
        }))
        .expect("alias preset should parse")
        .expect("alias preset should expand");

        assert_eq!(alias.spec, primary.spec);
        assert_eq!(alias.input, primary.input);
    }

    #[test]
    fn forced_agent_model_override_rejects_cross_model_preset() {
        let error = cross_model_verified_with_override(
            &json!({
                "preset": "cross_model_verified",
                "input": "fix the parser",
                "coding_model": "gpt-5.5-fast",
                "review_model": "claude-opus-4-8"
            }),
            Some("forced-model"),
        )
        .expect_err("forced single-model env disables cross-model preset")
        .to_string();

        assert!(error.contains("ZO_AGENT_MODEL"), "got {error}");
        assert!(error.contains("forced-model"), "got {error}");
    }

    #[test]
    fn unknown_preset_error_mentions_alias() {
        let error = expand_preset(&json!({ "preset": "unknown" }))
            .expect_err("unknown preset should fail")
            .to_string();
        assert!(error.contains("cross_model_verified"), "got {error}");
        assert!(error.contains("gpt_claude_verified"), "got {error}");
    }

    #[test]
    fn preset_rejects_missing_task() {
        let error = expand_preset(&json!({
            "preset": "cross_model_verified",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8"
        }))
        .expect_err("task input is required")
        .to_string();
        assert!(error.contains("input"), "got {error}");
    }

    #[test]
    fn preset_rejects_missing_models() {
        let error = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser"
        }))
        .expect_err("cross-model preset needs explicit models")
        .to_string();
        assert!(error.contains("coding_model"), "got {error}");
    }

    #[test]
    fn preset_rejects_same_coding_and_review_model() {
        let error = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": " gpt-5.5-fast "
        }))
        .expect_err("models must differ")
        .to_string();
        assert!(error.contains("must be different"), "got {error}");
    }

    #[test]
    fn design_model_routes_preflight_and_defaults_to_review_model() {
        let explicit = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "design_model": "claude-fable-5"
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        assert_eq!(
            explicit.spec["phases"][0]["model"],
            "claude-fable-5",
            "explicit design model must pin the preflight phase"
        );
        assert_eq!(
            explicit.spec["phases"][2]["model"],
            "claude-opus-4-8",
            "review must stay on the review model"
        );

        let defaulted = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8"
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        assert_eq!(
            defaulted.spec["phases"][0]["model"],
            "claude-opus-4-8",
            "absent design model must keep preflight on the review model"
        );
    }

    #[test]
    fn repair_model_routes_repair_phase_and_defaults_to_coding_model() {
        let explicit = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "repair_model": "gpt-5.6-sol"
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        assert_eq!(
            explicit.spec["phases"][3]["model"],
            "gpt-5.6-sol",
            "explicit repair model must pin the repair lane"
        );
        assert_eq!(
            explicit.spec["phases"][1]["model"],
            "gpt-5.5-fast",
            "implementation must stay on the coding model"
        );

        let defaulted = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8"
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        assert_eq!(
            defaulted.spec["phases"][3]["model"],
            "gpt-5.5-fast",
            "absent repair model must keep the repair lane on the coding model"
        );
    }

    #[test]
    fn preset_rejects_repair_model_equal_to_review_model() {
        let error = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "repair_model": " claude-opus-4-8 "
        }))
        .expect_err("repair lane must stay independent of the review model")
        .to_string();
        assert!(error.contains("repair_model"), "got {error}");
        assert!(error.contains("must be different"), "got {error}");
    }

    #[test]
    fn preset_prompts_require_docs_skill_and_visual_verification() {
        let expanded = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8"
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        let prompt = |index: usize| {
            expanded.spec["phases"][index]["prompt"]
                .as_str()
                .expect("phase prompt should be a string")
                .to_string()
        };
        assert!(
            prompt(1).contains("context7") && prompt(1).contains("frontend-design"),
            "implement prompt must require docs-first APIs and the design skill"
        );
        assert!(
            prompt(3).contains("context7") && prompt(3).contains("frontend-design"),
            "repair prompt must require docs-first APIs and the design skill"
        );
        assert!(
            prompt(4).contains("visual verification"),
            "final verification prompt must require visual evidence for UI changes"
        );
    }

    #[test]
    fn preset_defaults_blank_optional_models_to_documented_defaults() {
        let expanded = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "design_model": " ",
            "repair_model": ""
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        assert_eq!(
            expanded.spec["phases"][0]["model"],
            "claude-opus-4-8",
            "blank design model must fall back to the review model, not an empty pin"
        );
        assert_eq!(
            expanded.spec["phases"][3]["model"],
            "gpt-5.5-fast",
            "blank repair model must fall back to the coding model, not an empty pin"
        );
    }

    #[test]
    fn preset_preflight_design_brief_flows_to_implementer() {
        let expanded = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "redesign the dashboard",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8"
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        let preflight_schema = &expanded.spec["phases"][0]["schema"]["properties"];
        assert!(
            preflight_schema.get("design_plan").is_some(),
            "preflight schema must carry a design_plan field for UI tasks"
        );
        assert!(
            preflight_schema.get("acceptance_criteria").is_some(),
            "preflight schema must carry acceptance_criteria for UI tasks"
        );
        let preflight_prompt = expanded.spec["phases"][0]["prompt"]
            .as_str()
            .expect("preflight prompt");
        assert!(
            preflight_prompt.contains("design_plan")
                && preflight_prompt.contains("acceptance_criteria"),
            "preflight prompt must ask for the design brief on UI tasks"
        );
        let implement_prompt = expanded.spec["phases"][1]["prompt"]
            .as_str()
            .expect("implement prompt");
        assert!(
            implement_prompt.contains("design_plan")
                && implement_prompt.contains("acceptance_criteria"),
            "implement prompt must follow the preflight design brief"
        );
        let review_prompt = expanded.spec["phases"][2]["prompt"]
            .as_str()
            .expect("review prompt");
        assert!(
            review_prompt.contains("acceptance_criteria"),
            "review prompt must check the implementation against the acceptance criteria"
        );
    }

    #[test]
    fn preset_final_verification_hard_gates_on_verify_command() {
        let expanded = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "verify_command": "cargo test -p tools",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "repair_model": "gpt-5.6-sol",
            "max_rounds": 2
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        let final_phase = &expanded.spec["phases"][4];
        assert_eq!(
            final_phase["strategy"],
            "fix_until_verified",
            "final verification must be the engine's hard repair gate"
        );
        assert_eq!(
            final_phase["final_check"]["command"],
            "cargo test -p tools",
            "the hard gate must run the preset verify command"
        );
        assert_eq!(
            final_phase["fixer"]["model"],
            "gpt-5.6-sol",
            "verified findings must be fixed on the repair-model lane"
        );
        assert_eq!(
            final_phase["max_attempts"],
            2,
            "the hard gate attempt cap must follow max_rounds"
        );
        assert_eq!(
            final_phase["schema"]["required"],
            json!(["verdict"]),
            "the verifier must return a structured verdict the engine can classify"
        );
        let synthesize_prompt = expanded.spec["synthesize"]["prompt"]
            .as_str()
            .expect("synthesize prompt");
        assert!(
            synthesize_prompt.contains("FAILED"),
            "synthesis must report FAILED when findings stay blocked or the command stays red"
        );
    }

    #[test]
    fn preset_design_flag_requires_design_brief_fields() {
        let expanded = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "redesign the dashboard",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "design": true
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        assert_eq!(
            expanded.spec["phases"][0]["schema"]["required"],
            json!([
                "can_avoid_new_code",
                "minimal_approach",
                "design_plan",
                "acceptance_criteria"
            ]),
            "a design task must make the brief a required part of the preflight contract"
        );
        assert!(
            expanded.spec["phases"][0]["prompt"]
                .as_str()
                .expect("preflight prompt")
                .contains("REQUIRED"),
            "the preflight prompt must state the design brief is mandatory, not conditional"
        );
    }

    #[test]
    fn preset_design_defaults_keep_current_schema() {
        let expanded = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8"
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        assert_eq!(
            expanded.spec["phases"][0]["schema"]["required"],
            json!(["can_avoid_new_code", "minimal_approach"]),
            "a non-design task must not require the UI brief fields"
        );
    }

    #[test]
    fn preset_rejects_non_boolean_design_flag() {
        let error = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "fix the parser",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "design": "yes"
        }))
        .expect_err("`design` must not accept a truthy string")
        .to_string();
        assert!(error.contains("design"), "got {error}");
        assert!(error.contains("must be a boolean"), "got {error}");
    }

    #[test]
    fn preset_implement_echoes_design_contract_for_reviewer() {
        let expanded = expand_preset(&json!({
            "preset": "cross_model_verified",
            "input": "redesign the dashboard",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "design": true
        }))
        .expect("preset should parse")
        .expect("preset should expand");
        let implement_prompt = expanded.spec["phases"][1]["prompt"]
            .as_str()
            .expect("implement prompt");
        assert!(
            implement_prompt.contains("Design contract"),
            "the implementer must restate the brief so the reviewer's handoff carries it"
        );
        let review_prompt = expanded.spec["phases"][2]["prompt"]
            .as_str()
            .expect("review prompt");
        assert!(
            review_prompt.contains("Design contract"),
            "the reviewer must check the handoff section it actually receives"
        );
        assert!(
            review_prompt.contains("blocker"),
            "a missing Design contract section must itself be reportable as a blocker"
        );
    }
}
