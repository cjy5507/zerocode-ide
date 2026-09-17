//! Declarative workflow spec: parsing, validation, normalization.
//!
//! Two representations, on purpose:
//!
//! * **External** ([`WorkflowSpec`] and friends) is what the model writes —
//!   flat and lenient so a JSON spec is easy to author. Optional fields use
//!   `#[serde(default)]`; every container is `#[serde(deny_unknown_fields)]`
//!   so a typo'd key is a hard error instead of a silently-ignored field.
//! * **Internal** ([`NormalizedWorkflow`]) is what the engine runs — strict.
//!   Illegal states are made unrepresentable: a phase's input source collapses
//!   to a single [`PhaseSource`] enum (so "both `fanout` and `over`" cannot
//!   exist), and `repeat` collapses to [`RepeatPolicy`].
//!
//! Parsing and validation are deliberately separate. [`WorkflowSpec::from_value`]
//! only checks structure; [`WorkflowSpec::validate`] checks *meaning* and
//! reports exactly which phase's which field is wrong.

use serde::Deserialize;
use serde_json::Value;

use crate::ToolError;

use super::auto_lanes::infer_auto_lane_fanout;

// ---------------------------------------------------------------------------
// External representation (deserialized from the model's JSON)
// ---------------------------------------------------------------------------

/// A workflow exactly as written by the model. Flat + lenient.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub mode: WorkflowMode,
    #[serde(default)]
    pub phases: Vec<PhaseSpec>,
    #[serde(default)]
    pub budget: Option<BudgetSpec>,
    #[serde(default)]
    pub isolation: Isolation,
    #[serde(default)]
    pub apply: ApplyPolicy,
    #[serde(default)]
    pub synthesize: Option<SynthesizeSpec>,
    #[serde(default)]
    pub judge: Option<JudgeSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhaseSpec {
    #[serde(default)]
    pub id: String,
    /// Static fan-out list. `"$input"` is a sentinel expanded against the
    /// workflow input by the engine; not interpreted here.
    #[serde(default)]
    pub fanout: Option<Vec<String>>,
    /// Map over the results of an earlier phase (named by id).
    #[serde(default)]
    pub over: Option<String>,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub subagent_type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
    #[serde(default)]
    pub repeat: Option<RepeatSpec>,
    #[serde(default)]
    pub strategy: PhaseStrategy,
    #[serde(default)]
    pub validator: Option<RepairStepSpec>,
    #[serde(default)]
    pub fixer: Option<RepairStepSpec>,
    #[serde(default)]
    pub final_check: Option<FinalCheckSpec>,
    #[serde(default)]
    pub max_attempts: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PhaseStrategy {
    #[default]
    Default,
    FixUntilVerified,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepairStepSpec {
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub subagent_type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinalCheckSpec {
    #[serde(default)]
    pub command: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepeatSpec {
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
    #[serde(default)]
    pub until: Until,
    #[serde(default)]
    pub dedup_by: Option<String>,
}

fn default_max_rounds() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
// The `max_*` prefix is the semantic point: every field is a cap.
#[allow(clippy::struct_field_names)]
pub(crate) struct BudgetSpec {
    /// Cap on total agents the run may spawn. `None` (omitted) leaves the agent
    /// count unbounded — useful when only `max_output_tokens` is set. Optional so
    /// a budget can pin either limit, both, or (vacuously) neither.
    #[serde(default)]
    pub max_agents: Option<usize>,
    /// Cap on cumulative sub-agent output tokens. `None` (omitted) leaves output
    /// unbounded. Enforced *post-hoc* at phase/stage boundaries: an agent's token
    /// cost is only known after it runs, so this gates the next unit of work
    /// rather than clamping a fan-out the way `max_agents` does.
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    /// Cap on estimated cumulative sub-agent output **cost** in USD, derived from
    /// the active model's per-token output price. `None` leaves cost unbounded.
    /// Like `max_output_tokens` it is post-hoc — a run is stopped at the next work
    /// boundary once the estimate crosses the cap.
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SynthesizeSpec {
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub subagent_type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// A judging step: one agent ranks the completed candidates and picks a
/// winner, parallel to [`SynthesizeSpec`] but selecting rather than merging.
/// The prompt's `{candidates}` token is replaced with the labeled candidate
/// blocks; the agent answers via `StructuredOutput`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JudgeSpec {
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub subagent_type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// Execution strategy. `phases` (default) runs each phase behind a barrier;
/// `pipeline` streams items through stages (roadmap step 5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WorkflowMode {
    #[default]
    Phases,
    Pipeline,
}

/// Per-workflow isolation. `worktree` runs each agent in its own git worktree
/// so parallel editors cannot clobber one another; `none` shares the process cwd.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Isolation {
    #[default]
    None,
    Worktree,
}

/// What becomes of each isolated agent's file changes after the phase barrier.
/// `none` (default) discards them — the isolated read/analysis fan-out. With
/// `sequential` the engine merges each agent's diff back into the main working
/// tree (`git apply --3way`, in spawn order), turning the worktree pool into a
/// parallel *editing* primitive. Only meaningful under `isolation:"worktree"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ApplyPolicy {
    #[default]
    None,
    Sequential,
}

/// Repeat termination policy. `fixed` always runs `max_rounds`; `no_new`
/// stops early once a round adds no new `dedup_by` result (loop-until-dry);
/// `command_green` stops once a verification shell command exits zero — the
/// product TDD loop (implement → test → repeat-until-green). Not `Copy`: the
/// `command_green` variant carries an owned command string.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Until {
    #[default]
    Fixed,
    NoNew,
    /// Stop the repeat loop as soon as `command` exits 0 (else run `max_rounds`).
    /// External form: `{"command_green": {"command": "cargo test"}}`.
    CommandGreen {
        command: String,
    },
}

// ---------------------------------------------------------------------------
// Internal representation (validated, normalized — engine input)
// ---------------------------------------------------------------------------

/// Where a phase's items come from. Collapsing `fanout`/`over`/`single` into
/// one enum makes "both a fan-out and an over-mapping" unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PhaseSource {
    /// One agent per item. `"$input"` sentinels are still present; the engine
    /// expands them against the workflow input at run time.
    Fanout(Vec<String>),
    /// Map over the results of the named earlier phase (validated to exist and
    /// precede this phase, so the phase graph is a DAG).
    Over(String),
    /// A single agent, no fan-out.
    Single,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepeatPolicy {
    pub max_rounds: u32,
    pub until: Until,
    pub dedup_by: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct NormalizedPhase {
    pub id: String,
    pub source: PhaseSource,
    pub prompt: String,
    pub subagent_type: Option<String>,
    pub model: Option<String>,
    pub schema: Option<Value>,
    pub repeat: Option<RepeatPolicy>,
    pub repair_loop: Option<FixUntilVerified>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepairStep {
    pub prompt: String,
    pub subagent_type: Option<String>,
    pub model: Option<String>,
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalCheck {
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixUntilVerified {
    pub validator: Option<RepairStep>,
    pub fixer: RepairStep,
    pub final_check: FinalCheck,
    pub max_attempts: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct Synthesize {
    pub prompt: String,
    pub subagent_type: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Judge {
    pub prompt: String,
    pub subagent_type: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct NormalizedWorkflow {
    pub name: String,
    /// Optional human label, echoed into the report for traceability.
    pub description: String,
    pub mode: WorkflowMode,
    pub phases: Vec<NormalizedPhase>,
    pub max_agents: Option<usize>,
    /// Cumulative sub-agent output-token cap (`budget.max_output_tokens`).
    /// `None` leaves output unbounded.
    pub max_output_tokens: Option<u64>,
    /// Estimated cumulative output-cost cap in USD (`budget.max_cost_usd`).
    /// `None` leaves cost unbounded.
    pub max_cost_usd: Option<f64>,
    pub isolation: Isolation,
    pub apply: ApplyPolicy,
    pub synthesize: Option<Synthesize>,
    pub judge: Option<Judge>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

impl WorkflowSpec {
    /// Deserialize a spec from a raw JSON value, tolerating the
    /// stringified-JSON form some models emit (`"{...}"` instead of `{...}`).
    /// Mirrors the leniency of `coerce_agents` in `SpawnMultiAgent`.
    pub(crate) fn from_value(value: &Value) -> Result<Self, ToolError> {
        let parsed_string;
        let target = match value {
            Value::String(raw) => {
                parsed_string = crate::model_json::parse_model_json(raw).map_err(|err| {
                    invalid(format!(
                        "workflow spec was a string but not valid JSON: {err}"
                    ))
                })?;
                &parsed_string
            }
            other => other,
        };
        // The runtime dispatcher smuggles execution metadata into the same
        // top-level object as the spec (`__zo_tool_call_id`, see
        // `spawn_tool_execution_input`). Strip the reserved `__zo_` prefix
        // before the deny-unknown-fields parse, or the spec parser rejects the
        // runtime's own field and the whole Workflow call fails.
        let sanitized = match target {
            Value::Object(map) => Value::Object(
                map.iter()
                    .filter(|(key, _)| !key.starts_with("__zo_"))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            other => other.clone(),
        };
        serde_json::from_value::<Self>(sanitized)
            .map_err(|err| invalid(format!("invalid workflow spec: {err}")))
    }
}

// ---------------------------------------------------------------------------
// Validation + normalization
// ---------------------------------------------------------------------------

impl WorkflowSpec {
    /// Check the semantic invariants and normalize into a [`NormalizedWorkflow`].
    /// Each failure names the offending phase and field.
    pub(crate) fn validate(self) -> Result<NormalizedWorkflow, ToolError> {
        // (1) name + phases non-empty.
        if self.name.trim().is_empty() {
            return Err(invalid("workflow `name` must not be empty"));
        }
        if self.phases.is_empty() {
            return Err(invalid("workflow must declare at least one phase"));
        }
        // (6a) budget caps, when present, must be greater than 0 — a 0 cap would
        // forbid all work, which is never what the author meant.
        if let Some(budget) = &self.budget {
            if budget.max_agents == Some(0) {
                return Err(invalid("`budget.max_agents` must be greater than 0"));
            }
            if budget.max_output_tokens == Some(0) {
                return Err(invalid("`budget.max_output_tokens` must be greater than 0"));
            }
            // Reject non-positive *and* NaN (a bare `<= 0.0` would let NaN slip).
            if budget
                .max_cost_usd
                .is_some_and(|cost| cost <= 0.0 || cost.is_nan())
            {
                return Err(invalid("`budget.max_cost_usd` must be greater than 0"));
            }
        }

        let mut prior_ids: Vec<String> = Vec::with_capacity(self.phases.len());
        let mut phases: Vec<NormalizedPhase> = Vec::with_capacity(self.phases.len());

        for (idx, phase) in self.phases.into_iter().enumerate() {
            // (2) id non-empty + unique.
            let id = phase.id.trim().to_string();
            if id.is_empty() {
                return Err(invalid(format!("phase #{idx} is missing an `id`")));
            }
            if prior_ids.contains(&id) {
                return Err(invalid(format!("phase `id` `{id}` is not unique")));
            }
            // (5) prompt present.
            if phase.prompt.trim().is_empty() {
                return Err(invalid(format!("phase `{id}` is missing a `prompt`")));
            }
            // (3 + 4) input source — mutually exclusive, `over` must be a DAG edge.
            let source = normalize_source(&id, phase.fanout, phase.over, &phase.prompt, &prior_ids)?;
            // (6b + 7) repeat policy.
            let repeat = phase
                .repeat
                .map(|spec| normalize_repeat(&id, spec, phase.schema.is_some()))
                .transpose()?;
            let repair_loop = normalize_repair_loop(
                &id,
                phase.strategy,
                phase.validator,
                phase.fixer,
                phase.final_check,
                phase.max_attempts,
            )?;

            prior_ids.push(id.clone());
            phases.push(NormalizedPhase {
                id,
                source,
                prompt: phase.prompt,
                subagent_type: clean_opt(phase.subagent_type),
                model: clean_opt(phase.model),
                schema: phase.schema,
                repeat,
                repair_loop,
            });
        }

        // (8) pipeline-mode constraint: `repeat` is phases-only in this build.
        // The companion "first phase must not be `over`" clause needs no check
        // here — invariant 4 (the DAG edge) already forbids it, since phase 0
        // has no earlier phase to map over.
        if self.mode == WorkflowMode::Pipeline
            && phases
                .iter()
                .any(|phase| phase.repeat.is_some() || phase.repair_loop.is_some())
        {
            return Err(invalid(
                "pipeline mode does not support `repeat` or `fix_until_verified` (use `mode: \"phases\"`)",
            ));
        }

        // (9) merge-back only means something when agents are isolated — there is
        // no separate change-set to apply back without a per-agent worktree.
        if self.apply == ApplyPolicy::Sequential && self.isolation != Isolation::Worktree {
            return Err(invalid(
                "`apply: \"sequential\"` requires `isolation: \"worktree\"`",
            ));
        }

        let synthesize = self.synthesize.map(normalize_synthesize).transpose()?;
        let judge = self.judge.map(normalize_judge).transpose()?;

        // One budget yields all caps; each may be `None` (unbounded).
        let (max_agents, max_output_tokens, max_cost_usd) =
            self.budget.map_or((None, None, None), |budget| {
                (
                    budget.max_agents,
                    budget.max_output_tokens,
                    budget.max_cost_usd,
                )
            });

        Ok(NormalizedWorkflow {
            name: self.name.trim().to_string(),
            description: self.description,
            mode: self.mode,
            phases,
            max_agents,
            max_output_tokens,
            max_cost_usd,
            isolation: self.isolation,
            apply: self.apply,
            synthesize,
            judge,
        })
    }
}

/// Collapse `fanout`/`over` into a [`PhaseSource`], enforcing invariants 3 & 4.
fn normalize_source(
    id: &str,
    fanout: Option<Vec<String>>,
    over: Option<String>,
    prompt: &str,
    prior_ids: &[String],
) -> Result<PhaseSource, ToolError> {
    match (fanout, over) {
        (Some(_), Some(_)) => Err(invalid(format!(
            "phase `{id}`: `fanout` and `over` are mutually exclusive"
        ))),
        (Some(items), None) => {
            if items.is_empty() {
                return Err(invalid(format!("phase `{id}`: `fanout` must not be empty")));
            }
            Ok(PhaseSource::Fanout(items))
        }
        (None, Some(target)) => {
            let target = target.trim().to_string();
            if target.is_empty() {
                return Err(invalid(format!("phase `{id}`: `over` must name a phase")));
            }
            if target == id {
                return Err(invalid(format!(
                    "phase `{id}`: `over` cannot reference itself"
                )));
            }
            if !prior_ids.contains(&target) {
                return Err(invalid(format!(
                    "phase `{id}`: `over` references `{target}`, which is not an earlier phase"
                )));
            }
            Ok(PhaseSource::Over(target))
        }
        (None, None) => infer_auto_lane_fanout(prompt).map_or(Ok(PhaseSource::Single), |items| {
            Ok(PhaseSource::Fanout(items))
        }),
    }
}

/// Normalize a `repeat` block, enforcing invariants 6b & 7.
fn normalize_repeat(
    id: &str,
    spec: RepeatSpec,
    has_schema: bool,
) -> Result<RepeatPolicy, ToolError> {
    if spec.max_rounds == 0 {
        return Err(invalid(format!(
            "phase `{id}`: `repeat.max_rounds` must be at least 1"
        )));
    }
    let dedup_by = clean_opt(spec.dedup_by);
    match &spec.until {
        // `no_new` needs a key to tell "new" from "seen".
        Until::NoNew if dedup_by.is_none() => {
            return Err(invalid(format!(
                "phase `{id}`: `repeat.until = \"no_new\"` requires `dedup_by`"
            )));
        }
        // …and a key can only be read out of STRUCTURED output. Without a
        // `schema` the agent replies prose, `item_dedup_keys` fails to parse it
        // as JSON and returns nothing, `accumulate_round` files every item as
        // keyless, and `new_in_round` is 0 on the very first round — so the
        // loop-until-dry break fires immediately and the phase reports
        // `rounds: 1` AS A SUCCESS. That is the worst shape a stop condition
        // can fail in: it looks exactly like "one round was enough".
        //
        // The coupling was real but invisible: both engine tests for `no_new`
        // happen to declare a schema, so the requirement was pinned by
        // accident and documented nowhere.
        Until::NoNew if !has_schema => {
            return Err(invalid(format!(
                "phase `{id}`: `repeat.until = \"no_new\"` requires the phase's `schema` — \
                 `dedup_by` reads its key from structured output, and without one every \
                 result is keyless, which would stop the loop after a single round and \
                 report it as complete"
            )));
        }
        // `command_green` is meaningless without a command to run.
        Until::CommandGreen { command } if command.trim().is_empty() => {
            return Err(invalid(format!(
                "phase `{id}`: `repeat.until = \"command_green\"` requires a non-empty `command`"
            )));
        }
        _ => {}
    }
    Ok(RepeatPolicy {
        max_rounds: spec.max_rounds,
        until: spec.until,
        dedup_by,
    })
}

fn normalize_repair_loop(
    id: &str,
    strategy: PhaseStrategy,
    validator: Option<RepairStepSpec>,
    fixer: Option<RepairStepSpec>,
    final_check: Option<FinalCheckSpec>,
    max_attempts: Option<u32>,
) -> Result<Option<FixUntilVerified>, ToolError> {
    if strategy == PhaseStrategy::Default {
        if validator.is_some() || fixer.is_some() || final_check.is_some() || max_attempts.is_some() {
            return Err(invalid(format!(
                "phase `{id}`: repair-loop fields require `strategy: \"fix_until_verified\"`"
            )));
        }
        return Ok(None);
    }

    let fixer = fixer
        .map(|step| normalize_repair_step(id, "fixer", step))
        .transpose()?
        .ok_or_else(|| {
            invalid(format!(
                "phase `{id}`: `fixer.prompt` is required for `fix_until_verified`"
            ))
        })?;
    let final_check = final_check
        .map(|check| normalize_final_check(id, check))
        .transpose()?
        .ok_or_else(|| {
            invalid(format!(
                "phase `{id}`: `final_check.command` is required for `fix_until_verified`"
            ))
        })?;
    let validator = validator
        .map(|step| normalize_repair_step(id, "validator", step))
        .transpose()?;
    let max_attempts = max_attempts.unwrap_or(2);
    if max_attempts == 0 {
        return Err(invalid(format!(
            "phase `{id}`: `max_attempts` must be at least 1"
        )));
    }

    Ok(Some(FixUntilVerified {
        validator,
        fixer,
        final_check,
        max_attempts,
    }))
}

fn normalize_repair_step(
    phase_id: &str,
    field: &str,
    spec: RepairStepSpec,
) -> Result<RepairStep, ToolError> {
    if spec.prompt.trim().is_empty() {
        return Err(invalid(format!(
            "phase `{phase_id}`: `{field}.prompt` must not be empty"
        )));
    }
    Ok(RepairStep {
        prompt: spec.prompt,
        subagent_type: clean_opt(spec.subagent_type),
        model: clean_opt(spec.model),
        schema: spec.schema,
    })
}

fn normalize_final_check(id: &str, spec: FinalCheckSpec) -> Result<FinalCheck, ToolError> {
    if spec.command.trim().is_empty() {
        return Err(invalid(format!(
            "phase `{id}`: `final_check.command` must not be empty"
        )));
    }
    Ok(FinalCheck {
        command: spec.command,
    })
}

fn normalize_synthesize(spec: SynthesizeSpec) -> Result<Synthesize, ToolError> {
    if spec.prompt.trim().is_empty() {
        return Err(invalid("`synthesize.prompt` must not be empty"));
    }
    Ok(Synthesize {
        prompt: spec.prompt,
        subagent_type: clean_opt(spec.subagent_type),
        model: clean_opt(spec.model),
    })
}

fn normalize_judge(spec: JudgeSpec) -> Result<Judge, ToolError> {
    if spec.prompt.trim().is_empty() {
        return Err(invalid("`judge.prompt` must not be empty"));
    }
    Ok(Judge {
        prompt: spec.prompt,
        subagent_type: clean_opt(spec.subagent_type),
        model: clean_opt(spec.model),
    })
}

/// Trim an optional string and drop it when empty, so blank values never reach
/// the engine as `Some("")`.
fn clean_opt(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::InvalidInput(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Parse + validate in one shot, the way the tool surface does.
    fn normalize(value: &Value) -> Result<NormalizedWorkflow, ToolError> {
        WorkflowSpec::from_value(value).and_then(WorkflowSpec::validate)
    }

    fn err_message(value: &Value) -> String {
        normalize(value)
            .expect_err("expected validation failure")
            .to_string()
    }

    #[test]
    fn parses_and_normalizes_minimal_single_phase() {
        let wf = normalize(&json!({
            "name": "demo",
            "phases": [{ "id": "only", "prompt": "do the thing" }]
        }))
        .expect("valid");
        assert_eq!(wf.name, "demo");
        assert_eq!(wf.mode, WorkflowMode::Phases);
        assert_eq!(wf.isolation, Isolation::None);
        assert_eq!(wf.phases.len(), 1);
        assert_eq!(wf.phases[0].source, PhaseSource::Single);
        assert!(wf.max_agents.is_none());
    }

    #[test]
    fn normalizes_explicit_models_for_phases_repair_and_summary_agents() {
        let wf = normalize(&json!({
            "name": "models",
            "phases": [{
                "id": "fix",
                "prompt": "do the thing",
                "model": " gpt-5.5-fast ",
                "strategy": "fix_until_verified",
                "validator": {
                    "prompt": "verify",
                    "model": "claude-opus-4-8"
                },
                "fixer": {
                    "prompt": "repair",
                    "model": "gpt-5.5-fast"
                },
                "final_check": { "command": "cargo test -p tools" }
            }],
            "synthesize": { "prompt": "sum {all}", "model": "claude-opus-4-8" },
            "judge": { "prompt": "pick {candidates}", "model": "claude-opus-4-8" }
        }))
        .expect("valid");

        assert_eq!(wf.phases[0].model.as_deref(), Some("gpt-5.5-fast"));
        let loop_spec = wf.phases[0].repair_loop.as_ref().expect("repair loop");
        assert_eq!(
            loop_spec.validator.as_ref().and_then(|step| step.model.as_deref()),
            Some("claude-opus-4-8")
        );
        assert_eq!(loop_spec.fixer.model.as_deref(), Some("gpt-5.5-fast"));
        assert_eq!(
            wf.synthesize.as_ref().and_then(|step| step.model.as_deref()),
            Some("claude-opus-4-8")
        );
        assert_eq!(
            wf.judge.as_ref().and_then(|step| step.model.as_deref()),
            Some("claude-opus-4-8")
        );
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let message = WorkflowSpec::from_value(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "prompt": "x" }],
            "bogus": 1
        }))
        .expect_err("deny_unknown_fields should reject")
        .to_string();
        assert!(message.contains("unknown field"), "got: {message}");
        assert!(message.contains("bogus"), "got: {message}");
    }

    #[test]
    fn tolerates_runtime_smuggled_zo_metadata_fields() {
        // The runtime dispatcher injects `__zo_tool_call_id` (agent
        // attribution) into the same top-level object as the spec. The parser
        // must strip the reserved `__zo_` prefix instead of rejecting its
        // own runtime's field — this exact failure shipped as
        // "invalid workflow spec: unknown field `__zo_tool_call_id`".
        let wf = normalize(&json!({
            "name": "demo",
            "phases": [{ "id": "only", "prompt": "do the thing" }],
            "__zo_tool_call_id": "call_123"
        }))
        .expect("smuggled runtime metadata must not fail the spec parse");
        assert_eq!(wf.name, "demo");
    }

    #[test]
    fn rejects_unknown_phase_field() {
        let message = WorkflowSpec::from_value(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "prompt": "x", "fanoot": ["typo"] }]
        }))
        .expect_err("deny_unknown_fields should reject")
        .to_string();
        assert!(message.contains("unknown field"), "got: {message}");
    }

    #[test]
    fn coerces_stringified_spec() {
        let inner = json!({
            "name": "demo",
            "phases": [{ "id": "only", "prompt": "do it" }]
        })
        .to_string();
        let wf = normalize(&Value::String(inner)).expect("stringified spec should parse");
        assert_eq!(wf.name, "demo");
    }

    #[test]
    fn rejects_empty_name() {
        assert!(err_message(&json!({
            "name": "  ",
            "phases": [{ "id": "p", "prompt": "x" }]
        }))
        .contains("`name` must not be empty"));
    }

    #[test]
    fn rejects_empty_phases() {
        assert!(
            err_message(&json!({ "name": "demo", "phases": [] })).contains("at least one phase")
        );
    }

    #[test]
    fn rejects_missing_phase_id() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{ "prompt": "x" }]
        }))
        .contains("missing an `id`"));
    }

    #[test]
    fn rejects_duplicate_phase_id() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [
                { "id": "dup", "prompt": "a" },
                { "id": "dup", "prompt": "b" }
            ]
        }))
        .contains("is not unique"));
    }

    #[test]
    fn rejects_fanout_and_over_together() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [
                { "id": "first", "prompt": "a" },
                { "id": "second", "prompt": "b", "fanout": ["x"], "over": "first" }
            ]
        }))
        .contains("mutually exclusive"));
    }

    #[test]
    fn rejects_over_forward_reference() {
        // `over` points at a phase that has not appeared yet → not a DAG edge.
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [
                { "id": "uses", "prompt": "a", "over": "later" },
                { "id": "later", "prompt": "b" }
            ]
        }))
        .contains("not an earlier phase"));
    }

    #[test]
    fn rejects_over_self_reference() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{ "id": "loop", "prompt": "a", "over": "loop" }]
        }))
        .contains("cannot reference itself"));
    }

    #[test]
    fn rejects_missing_prompt() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "prompt": "   " }]
        }))
        .contains("missing a `prompt`"));
    }

    #[test]
    fn rejects_zero_budget() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "prompt": "x" }],
            "budget": { "max_agents": 0 }
        }))
        .contains("greater than 0"));
    }

    #[test]
    fn rejects_zero_output_token_budget() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "prompt": "x" }],
            "budget": { "max_output_tokens": 0 }
        }))
        .contains("greater than 0"));
    }

    #[test]
    fn normalizes_output_token_budget_alone_and_alongside_agents() {
        // Token-only budget: the agent count stays unbounded.
        let token_only = normalize(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "fanout": ["a"], "prompt": "do {item}" }],
            "budget": { "max_output_tokens": 50_000 }
        }))
        .expect("valid");
        assert_eq!(token_only.max_agents, None);
        assert_eq!(token_only.max_output_tokens, Some(50_000));

        // Both caps pinned at once.
        let both = normalize(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "fanout": ["a"], "prompt": "do {item}" }],
            "budget": { "max_agents": 8, "max_output_tokens": 200_000 }
        }))
        .expect("valid");
        assert_eq!(both.max_agents, Some(8));
        assert_eq!(both.max_output_tokens, Some(200_000));

        // No budget at all: both unbounded.
        let none = normalize(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "fanout": ["a"], "prompt": "do {item}" }]
        }))
        .expect("valid");
        assert_eq!(none.max_agents, None);
        assert_eq!(none.max_output_tokens, None);
    }

    #[test]
    fn rejects_zero_repeat_rounds() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{
                "id": "p", "prompt": "x",
                "fanout": ["round"],
                "repeat": { "max_rounds": 0 }
            }]
        }))
        .contains("at least 1"));
    }

    #[test]
    fn rejects_no_new_without_dedup() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{
                "id": "p", "prompt": "x",
                "fanout": ["round"],
                "repeat": { "max_rounds": 3, "until": "no_new" }
            }]
        }))
        .contains("requires `dedup_by`"));
    }

    #[test]
    fn normalizes_fanout_over_single_and_input_source() {
        let wf = normalize(&json!({
            "name": "review",
            "mode": "phases",
            "phases": [
                { "id": "review", "fanout": ["correctness", "security"], "prompt": "review {item}" },
                { "id": "verify", "over": "review", "prompt": "refute {item}" }
            ],
            "budget": { "max_agents": 20 },
            "synthesize": { "prompt": "combine {all}" }
        }))
        .expect("valid");
        assert_eq!(
            wf.phases[0].source,
            PhaseSource::Fanout(vec!["correctness".into(), "security".into()])
        );
        assert_eq!(wf.phases[1].source, PhaseSource::Over("review".into()));
        assert_eq!(wf.max_agents, Some(20));
        assert!(wf.synthesize.is_some());
    }

    #[test]
    fn apply_defaults_to_none() {
        let wf = normalize(&json!({
            "name": "demo",
            "isolation": "worktree",
            "phases": [{ "id": "p", "fanout": ["a"], "prompt": "edit {item}" }]
        }))
        .expect("valid");
        assert_eq!(wf.isolation, Isolation::Worktree);
        assert_eq!(wf.apply, ApplyPolicy::None);
    }

    #[test]
    fn accepts_sequential_apply_with_worktree() {
        let wf = normalize(&json!({
            "name": "demo",
            "isolation": "worktree",
            "apply": "sequential",
            "phases": [{ "id": "p", "fanout": ["a", "b"], "prompt": "edit {item}" }]
        }))
        .expect("valid");
        assert_eq!(wf.apply, ApplyPolicy::Sequential);
    }

    #[test]
    fn rejects_sequential_apply_without_worktree() {
        // `apply` with the default `isolation:"none"` has no isolated change-set
        // to merge — reject loudly rather than silently no-op.
        assert!(err_message(&json!({
            "name": "demo",
            "apply": "sequential",
            "phases": [{ "id": "p", "prompt": "x" }]
        }))
        .contains("requires `isolation: \"worktree\"`"));
    }

    #[test]
    fn normalizes_judge_block() {
        let wf = normalize(&json!({
            "name": "compare",
            "phases": [
                { "id": "gen", "fanout": ["mvp", "robust"], "prompt": "solve {item}" }
            ],
            "judge": { "prompt": "pick the best:\n{candidates}" }
        }))
        .expect("valid");
        let judge = wf.judge.expect("judge present");
        assert_eq!(judge.prompt, "pick the best:\n{candidates}");
        assert!(judge.subagent_type.is_none());
    }

    #[test]
    fn rejects_empty_judge_prompt() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "fanout": ["x"], "prompt": "go" }],
            "judge": { "prompt": "" }
        }))
        .contains("`judge.prompt` must not be empty"));
    }

    #[test]
    fn rejects_unknown_judge_field() {
        // deny_unknown_fields guards the judge block like every other container.
        assert!(WorkflowSpec::from_value(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "fanout": ["x"], "prompt": "go" }],
            "judge": { "prompt": "pick", "weight": 3 }
        }))
        .is_err());
    }

    #[test]
    fn accepts_dollar_input_fanout_sentinel() {
        // `$input` passes validation untouched; expansion is the engine's job.
        let wf = normalize(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "fanout": ["$input"], "prompt": "handle {item}" }]
        }))
        .expect("valid");
        assert_eq!(
            wf.phases[0].source,
            PhaseSource::Fanout(vec!["$input".into()])
        );
    }

    #[test]
    fn first_phase_over_is_always_rejected() {
        // Invariant 4 (the DAG edge) already forbids a first phase from mapping
        // `over` anything — there is no earlier phase. Holds in any mode.
        assert!(err_message(&json!({
            "name": "demo",
            "mode": "pipeline",
            "phases": [{ "id": "a", "over": "whatever", "prompt": "x" }]
        }))
        .contains("not an earlier phase"));
    }

    #[test]
    fn pipeline_with_fanout_first_phase_is_valid() {
        let wf = normalize(&json!({
            "name": "demo",
            "mode": "pipeline",
            "phases": [
                { "id": "a", "fanout": ["x", "y"], "prompt": "stage one {item}" },
                { "id": "b", "over": "a", "prompt": "stage two {item}" }
            ]
        }))
        .expect("valid pipeline spec");
        assert_eq!(wf.mode, WorkflowMode::Pipeline);
        assert_eq!(wf.phases.len(), 2);
    }

    #[test]
    fn pipeline_rejects_repeat() {
        assert!(err_message(&json!({
            "name": "demo",
            "mode": "pipeline",
            "phases": [{
                "id": "a", "prompt": "x",
                "fanout": ["one"],
                "repeat": { "max_rounds": 2 }
            }]
        }))
        .contains("pipeline mode does not support `repeat`"));
    }

    #[test]
    fn rejects_unknown_mode_variant() {
        let message = WorkflowSpec::from_value(&json!({
            "name": "demo",
            "mode": "turbo",
            "phases": [{ "id": "p", "prompt": "x" }]
        }))
        .expect_err("unknown enum variant")
        .to_string();
        assert!(message.contains("unknown variant"), "got: {message}");
    }

    #[test]
    fn rejects_empty_synthesize_prompt() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{ "id": "p", "prompt": "x" }],
            "synthesize": { "prompt": "" }
        }))
        .contains("`synthesize.prompt` must not be empty"));
    }

    #[test]
    fn normalizes_repeat_policy() {
        let wf = normalize(&json!({
            "name": "demo",
            "phases": [{
                "id": "hunt", "prompt": "find {seen}",
                "fanout": ["round"],
                "schema": { "type": "object" },
                "repeat": { "max_rounds": 4, "until": "no_new", "dedup_by": "bugs[].title" }
            }]
        }))
        .expect("valid");
        let repeat = wf.phases[0].repeat.as_ref().expect("repeat present");
        assert_eq!(repeat.max_rounds, 4);
        assert_eq!(repeat.until, Until::NoNew);
        assert_eq!(repeat.dedup_by.as_deref(), Some("bugs[].title"));
    }

    /// `dedup_by` reads its key out of STRUCTURED output, so `no_new` without a
    /// `schema` cannot work — and it fails in the worst possible shape: every
    /// result is keyless, `new_in_round` is 0 on the first round, the
    /// loop-until-dry break fires immediately, and the phase reports
    /// `rounds: 1` as a success. Indistinguishable from "one round sufficed".
    ///
    /// This test's own fixture used to omit `schema` and pass, which is how the
    /// coupling stayed invisible: the spec layer pinned a combination the
    /// engine cannot execute.
    #[test]
    fn rejects_no_new_without_the_schema_its_dedup_key_is_read_from() {
        let message = err_message(&json!({
            "name": "demo",
            "phases": [{
                "id": "hunt", "prompt": "find {seen}",
                "repeat": { "max_rounds": 4, "until": "no_new", "dedup_by": "bugs[].title" }
            }]
        }));
        assert!(
            message.contains("requires the phase's `schema`"),
            "the error must name the missing field: {message}"
        );
    }

    #[test]
    fn normalizes_command_green_repeat() {
        // The TDD-loop form: repeat the phase until the verification command
        // exits 0. No `dedup_by` required (orthogonal stop condition).
        let wf = normalize(&json!({
            "name": "tdd",
            "phases": [{
                "id": "impl", "prompt": "implement {seen}",
                "repeat": { "max_rounds": 5, "until": { "command_green": { "command": "cargo test" } } }
            }]
        }))
        .expect("valid");
        let repeat = wf.phases[0].repeat.as_ref().expect("repeat present");
        assert_eq!(repeat.max_rounds, 5);
        assert_eq!(
            repeat.until,
            Until::CommandGreen {
                command: "cargo test".into()
            }
        );
        assert!(repeat.dedup_by.is_none());
    }

    #[test]
    fn rejects_command_green_with_empty_command() {
        assert!(err_message(&json!({
            "name": "demo",
            "phases": [{
                "id": "p", "prompt": "x",
                "repeat": { "max_rounds": 3, "until": { "command_green": { "command": "  " } } }
            }]
        }))
        .contains("requires a non-empty `command`"));
    }

    #[test]
    fn rejects_command_green_missing_command_field() {
        // `command` is required inside the variant — a missing/typo'd key is a
        // hard parse error, not a silent default.
        assert!(WorkflowSpec::from_value(&json!({
            "name": "demo",
            "phases": [{
                "id": "p", "prompt": "x",
                "repeat": { "max_rounds": 3, "until": { "command_green": {} } }
            }]
        }))
        .is_err());
    }
}
