//! `Workflow` tool — declarative multi-phase agent orchestration.
//!
//! Responsibilities are split SRP-style across three files:
//! * [`spec`] — spec types, serde parsing, validation, normalization.
//! * [`engine`] — phase execution over an injected [`engine::AgentBackend`].
//! * this module — the tool surface: the [`ToolSpec`], the dispatch entry
//!   [`run_workflow`], and the production [`LiveBackend`] that binds the engine
//!   to zo's live agent primitives (`execute_agent` + `wait_for_agent_completions`).

mod auto_lanes;
mod cache;
// `pub(crate)`: Phase 4 verdict widening reuses the engine's existing
// verdict-attribution recorder (`engine::attribution`) and structured-verdict
// classifier (`engine::items::semantic_verdict`) from the general single-agent
// spawn path (`misc_tools::agent_tools::spawn`), a sibling module tree, not a
// descendant of `workflow_tools` — so both need to be nameable from outside.
pub(crate) mod engine;
mod event_store;
mod inspector;
mod library;
mod presets;
pub(crate) use presets::verdict_schema;
mod progress;
mod skill_projection;
mod spec;
mod wait;
pub(crate) mod worktree;

// Phase-6: best-effort artifact-metadata recording into the SQLite store, called
// from `crate::artifacts` after the content-addressed bytes are written.
pub(crate) use event_store::record_artifact_meta;

pub use engine::request_foreground_workflow_cancel;
pub use progress::{
    event_log_terminal_status, event_phase_statuses, event_timeline_lines, read_event_log,
    EventPhase, WorkflowEventKind, WorkflowEventRecord,
};

use std::time::Duration;

use core_types::helper_run::HelperRun;
use runtime::{
    permission_enforcer::{EnforcementResult, PermissionEnforcer},
    PermissionMode, PermissionPolicy,
};
use serde_json::{json, Value};

use crate::misc_tools::{AgentCompletion, AgentInput};
use crate::{ToolError, ToolSpec};
use engine::{AgentBackend, RunOptions};
use spec::{ApplyPolicy, Isolation, NormalizedWorkflow, PhaseSource, WorkflowMode, WorkflowSpec};

const WORKFLOW_DESCRIPTION: &str = "Run a declarative, multi-phase agent workflow and return a structured report. \
Generalizes SpawnMultiAgent from a single flat fan-out into multi-phase fan-out → reduce → synthesize, with the intermediate results held in the engine rather than your context. \
Input is either the spec directly, {\"spec\": <spec>, \"input\": <any>} where `input` is the workflow argument (substituted as {input}), or {\"preset\":\"cross_model_verified\", \"input\": <task>, \"coding_model\": <model>, \"review_model\": <different-model>, \"design_model\"?: <model>, \"repair_model\"?: <second-coding-model>, \"design\"?: true, \"verify_command\"?: <cmd>, \"max_rounds\"?: 3} for the built-in cross-model verification loop (`ZO_AGENT_MODEL` must be unset); add \"resumeFromRunId\" (a prior run's id) alongside `spec` or `preset` to replay completed phases from cache after editing the request. \
Spec: {name, description?, mode?, phases:[...], budget?, synthesize?, isolation?, apply?}. \
A phase is {id, prompt, fanout? | over?, subagent_type?, model?, schema?}: omit both `fanout` and `over` for one agent; `model` pins that phase's agents to an explicit model while `subagent_type` still selects the agent harness/role; `fanout` is a string array (one agent per item — the value \"$input\" expands an array input); `over`:\"<earlier-phase-id>\" runs one agent per completed result of that phase. \
Prompt tokens: {item}, {index}, {input}. An optional `schema` (JSON schema) makes each agent reply JSON, extracted into `structured` (one retry on failure, raw preserved otherwise). \
`budget`:{max_agents} caps total agents; `synthesize`:{prompt} runs a final agent over all results ({all}). \
A phase may set `repeat`:{max_rounds, until} to re-run itself with prior rounds' results substituted as {seen}: `until` is \"fixed\" (always max_rounds), \"no_new\" (stop once a round adds no new `dedup_by` result — loop-until-dry), or {\"command_green\":{\"command\":\"cargo test\"}} (stop once that shell command exits 0, else run max_rounds — the implement→test→repeat-until-green loop; the command runs in the working tree). \
`mode` is \"phases\" (default) or \"pipeline\": phases run sequentially behind a barrier (items within a phase run in parallel); pipeline streams each first-phase item through all later phases as an independent chain (stage k receives that item's stage k-1 result as {item}, no cross-item barrier). \
`isolation` is \"none\" (default) or \"worktree\" (each agent runs in its own git worktree so parallel editors never clobber each other); with \"worktree\" set `apply`:\"sequential\" to merge every agent's diff back into the working tree (git apply --3way, in spawn order) — the default \"none\" discards the isolated changes (read/analysis fan-out). \
For an implementation request big enough to warrant a workflow at all (multiple files or subsystems — one file or one module is direct solo work, not a workflow), build an efficient implement→verify pipeline: the `implement` agent starts with the required file inspection and local planning, actually edits files (use isolation \"worktree\"+apply \"sequential\" only when several edit in parallel; otherwise the default \"none\" edits the working tree directly), then a `verify` phase with `repeat`:{until:{command_green:{command:\"<your test command>\"}}} — produce real changes and prove they pass, never an analysis-only summary. Tell the verifier to run the requested comprehensive suite once when it passes; repeat the identical suite only after a fix or an inconclusive/unstable result. For one dependent implementation lane, do not add a standalone analysis phase; reserve one for an artifact consumed by multiple implementers. Do not set `synthesize` for a single implement→verify chain because the structured report already returns both results; synthesize only when merging multiple independent or competing outputs. \
Use this for multi-stage work; use SpawnMultiAgent for a single flat fan-out. \
Author a workflow only for dependent multi-phase work at real scale — a whole-repo refactor, a multi-subsystem migration, an audit the user asked to make exhaustive — never for a bounded fix, a routine question, or a document. Documentation and prose take direct writing with at most one review pass, not a workflow or a repair loop around subjective prose criteria.";

const WORKFLOW_VALIDATE_DESCRIPTION: &str = "Validate and dry-run preview a Workflow spec without executing the workflow, spawning agents, running commands, creating worktrees, or modifying files. \
Accepts the same input shapes as Workflow: a direct spec, {\"spec\": <spec>, \"input\": <any>}, or built-in preset forms such as {\"preset\":\"cross_model_verified\", ...}. \
Returns normalized preview JSON after parse and semantic validation.";

/// The `Workflow` tool spec. Spawns sub-agents, so it carries the same
/// `DangerFullAccess` requirement as `SpawnMultiAgent`; the sub-agents it
/// launches inherit `agent_permission_policy` + their `subagent_type`
/// allowlist unchanged — a workflow never widens permissions.
pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    let spec_properties = workflow_spec_properties();
    let mut top_properties = spec_properties.clone();
    insert_workflow_top_properties(&mut top_properties, &spec_properties);
    let input_schema = workflow_input_schema(&top_properties);

    vec![
        ToolSpec {
            name: "Workflow",
            description: WORKFLOW_DESCRIPTION,
            input_schema: input_schema.clone(),
            // Spawning is unprivileged — workflow agents' enforcers are
            // clamped to the parent's active mode (`clamped_spawn_mode`).
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "WorkflowValidate",
            description: WORKFLOW_VALIDATE_DESCRIPTION,
            input_schema,
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "WorkflowLibrary",
            description: "Save, list, show, or delete validated Workflow specs in Zo's stored workflow library. Saved specs can later be run or validated by passing {\"library\": <name>} to Workflow or WorkflowValidate.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["save", "list", "show", "delete"] },
                    "name": { "type": "string", "description": "Lowercase workflow-library name: [a-z0-9][a-z0-9_-]*, max 64 chars." },
                    "spec": { "type": "object", "additionalProperties": true, "description": "WorkflowSpec JSON object required for action=save." },
                    "overwrite": { "type": "boolean", "description": "Required true to replace an existing saved workflow." }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "WorkflowRuns",
            description: "Inspect stored workflow run event logs without modifying them. With no run_id, lists recent runs; with run_id, returns a timeline summary and bounded raw event tail.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "Workflow run id to inspect. Omit to list recent runs." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Recent-run list limit (default 20, capped at 100)." }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "WorkflowSkillProject",
            description: "Project a saved WorkflowLibrary entry into a proposed Zo skill draft at .zo/skills/<slug>/SKILL.md. The draft remains state: proposed and must be approved with SkillReview before use.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "WorkflowLibrary entry name to project." },
                    "slug": { "type": "string", "description": "Optional skill slug; defaults to the library name." },
                    "description": { "type": "string", "description": "Optional skill description; defaults to the saved workflow description." },
                    "update": { "type": "boolean", "description": "Replace an existing draft and bump version; defaults false." }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
    ]
}

fn workflow_input_schema(top_properties: &Value) -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "description": r#"A workflow spec, {"spec": <spec>, "input": <any>}, or {"preset": "cross_model_verified", "input": <task>, "coding_model": <model>, "review_model": <different-model>} where `input` substitutes as {input}."#,
        "properties": top_properties
    })
}
fn workflow_spec_properties() -> Value {
    json!({
        "name": { "type": "string", "minLength": 1, "description": "Short workflow name." },
        "description": { "type": "string" },
        "mode": { "type": "string", "enum": ["phases", "pipeline"],
            "description": "phases (default, barrier between phases) or pipeline (per-item streaming through stages)." },
        "phases": { "type": "array", "minItems": 1, "items": workflow_phase_schema(),
            "description": "Ordered phases; each phase needs an `id` and a `prompt`." },
        "budget": { "type": "object",
            "description": "{max_agents?, max_output_tokens?} caps (each > 0 when present)." },
        "synthesize": { "type": "object",
            "description": "{prompt} runs a final agent over all results ({all})." },
        "isolation": { "type": "string", "enum": ["none", "worktree"] },
        "apply": { "type": "string", "enum": ["none", "sequential"] }
    })
}

fn workflow_phase_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "required": ["id", "prompt"],
        "properties": {
            "id": { "type": "string", "minLength": 1,
                "description": "Unique phase id; a later phase's `over` references it." },
            "prompt": { "type": "string", "minLength": 1,
                "description": "Required. The agent instruction for this phase. Tokens: {item}, {index}, {input}, {seen}." },
            "fanout": { "type": "array", "items": { "type": "string" },
                "description": "One agent per item (\"$input\" expands an array input). Mutually exclusive with `over`." },
            "over": { "type": "string",
                "description": "Earlier phase id: one agent per its completed result. Mutually exclusive with `fanout`." },
            "subagent_type": { "type": "string" },
            "model": { "type": "string",
                "description": "Optional semantic model alias or explicit provider/model pin for this phase; prefer versionless aliases and never infer a concrete release id from stale knowledge. Keeps `subagent_type` as the harness/role selector. In direct specs, `ZO_AGENT_MODEL` still forces all sub-agents if set." },
            "schema": { "type": "object",
                "description": "JSON schema → each agent replies JSON, extracted into `structured`." },
            "repeat": { "type": "object",
                "additionalProperties": false,
                "description": "Re-runs this phase, with prior rounds' results relayed to the next round (as {seen} when the prompt names it, appended by the engine when it does not).",
                "properties": {
                    "max_rounds": { "type": "integer", "minimum": 1,
                        "description": "Upper bound on rounds. A value of 1 cannot repeat." },
                    "until": {
                        "description": "\"fixed\" (always max_rounds), \"no_new\" (loop-until-dry: stop once a round adds no new `dedup_by` key — REQUIRES both `dedup_by` and the phase's `schema`), or {\"command_green\":{\"command\":\"cargo test\"}} (stop once that shell command exits 0).",
                        "anyOf": [
                            { "type": "string", "enum": ["fixed", "no_new"] },
                            { "type": "object", "additionalProperties": false,
                              "required": ["command_green"],
                              "properties": { "command_green": { "type": "object",
                                  "additionalProperties": false,
                                  "required": ["command"],
                                  "properties": { "command": { "type": "string" } } } } }
                        ] },
                    "dedup_by": { "type": "string",
                        "description": "JSON path into each agent's structured output (`bugs[].title`, `id`, `a.b`) identifying one result. Required by until:\"no_new\", which counts a round as dry when it adds no unseen key." }
                } }
        }
    })
}

fn insert_workflow_top_properties(top_properties: &mut Value, spec_properties: &Value) {
    let Some(map) = top_properties.as_object_mut() else {
        return;
    };
    map.insert(
        "spec".to_string(),
        json!({
            "type": "object",
            "additionalProperties": true,
            "description": "Alternative: the workflow spec, with a sibling `input`.",
            "properties": spec_properties.clone()
        }),
    );
    map.insert(
        "input".to_string(),
        json!({ "description": "Workflow argument, substituted as {input}." }),
    );
    map.insert(
        "library".to_string(),
        json!({
            "type": "string",
            "description": "Stored WorkflowLibrary entry name. Mutually exclusive with `spec` and `preset`; sibling `input` is passed as the workflow argument."
        }),
    );
    map.insert(
        "resumeFromRunId".to_string(),
        json!({
            "type": "string",
            "description": "Resume a prior run's phase cache even though the spec/input changed (edited-spec resume): unchanged completed phases replay instantly, the rest run live. Only honored alongside `spec` or `preset`."
        }),
    );
    insert_preset_properties(map);
}

/// The `cross_model_verified` / `gpt_claude_verified` knobs. Split out of
/// [`insert_workflow_top_properties`] so the preset's input surface lives in one
/// place instead of trailing the generic spec/input properties.
fn insert_preset_properties(map: &mut serde_json::Map<String, Value>) {
    map.insert(
        "preset".to_string(),
        json!({
            "type": "string",
            "enum": ["cross_model_verified", "gpt_claude_verified"],
            "description": "Built-in workflow preset. `cross_model_verified` expands to preflight (minimality + UI design brief) → coding-role implement → reviewer-role critique → repair until `verify_command` passes in the main working tree → verifier-role final audit (a fix_until_verified hard gate: its fixer runs on `repair_model`, and a red `verify_command` ends as a blocked `final-check` finding). The cross axis is implementation model vs review/verification model; `ZO_AGENT_MODEL` must be unset. Alias: `gpt_claude_verified`."
        }),
    );
    map.insert(
        "verify_command".to_string(),
        json!({
            "type": "string",
            "description": "Verification command for `cross_model_verified`/`gpt_claude_verified`; defaults to `cargo check --workspace --all-targets`."
        }),
    );
    map.insert(
        "verification_command".to_string(),
        json!({
            "type": "string",
            "description": "Alias for `verify_command`."
        }),
    );
    map.insert(
        "coding_model".to_string(),
        json!({
            "type": "string",
            "description": "Required explicit implementation model for `cross_model_verified`/`gpt_claude_verified`. Must differ from `review_model`; swap these fields to let Claude code and GPT review."
        }),
    );
    map.insert(
        "review_model".to_string(),
        json!({
            "type": "string",
            "description": "Required explicit reviewer model for `cross_model_verified`/`gpt_claude_verified`. Must differ from `coding_model`; used by preflight/review unless more specific models are set."
        }),
    );
    map.insert(
        "design_model".to_string(),
        json!({
            "type": "string",
            "description": "Optional explicit preflight/design model for `cross_model_verified`/`gpt_claude_verified`; defaults to `review_model`."
        }),
    );
    map.insert(
        "repair_model".to_string(),
        json!({
            "type": "string",
            "description": "Optional explicit repair-lane model for `cross_model_verified`/`gpt_claude_verified` (a second implementer fixing review findings; also the final-audit fixer lane); defaults to `coding_model` and must differ from `review_model`."
        }),
    );
    map.insert(
        "design".to_string(),
        json!({
            "type": "boolean",
            "description": "Set true for user-facing UI/UX work in `cross_model_verified`/`gpt_claude_verified`: the preflight design brief (`design_plan` + `acceptance_criteria`) becomes a required field of that phase's schema, the implementer must restate it as a `Design contract` section, and the reviewer checks the result against it. Defaults to false."
        }),
    );
    map.insert(
        "verification_model".to_string(),
        json!({
            "type": "string",
            "description": "Optional explicit final verifier model for `cross_model_verified`/`gpt_claude_verified`; defaults to `review_model` when set."
        }),
    );
    map.insert(
        "synthesis_model".to_string(),
        json!({
            "type": "string",
            "description": "Optional explicit synthesis model for `cross_model_verified`/`gpt_claude_verified`; defaults to `review_model` when set."
        }),
    );
    map.insert(
        "max_rounds".to_string(),
        json!({
            "type": "integer",
            "minimum": 1,
            "description": "Max repair rounds for `cross_model_verified`/`gpt_claude_verified`; defaults to 3."
        }),
    );
}

/// Dispatch entry: parse + validate the spec, then run it on the live backend.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_workflow(
    input: &Value,
    parent_model: Option<&str>,
    parent_model_pinned: bool,
    hook_config: &runtime::RuntimeHookConfig,
    parent_session_id: Option<&str>,
    registry: Option<std::sync::Arc<crate::misc_tools::AgentRegistry>>,
    mcp_passthrough: Option<crate::registry::McpPassthrough>,
    permission_enforcer: Option<&PermissionEnforcer>,
    parent_permission_mode: Option<runtime::PermissionMode>,
) -> Result<String, ToolError> {
    // Key a resume cache by the (spec, input) pair so an identical re-run
    // replays completed phases from `.zo/workflows/<run_id>.json` instead of
    // re-spawning them. A missing store dir disables caching (no error).
    let prepared = prepare_workflow_input(input)?;
    let run_id = resolve_run_id(input, &prepared.spec_value, &prepared.workflow_input);
    let file_cache = cache::FileCache::resolve(run_id.clone());
    let semantic_cache = cache::FileSemanticCache::resolve(parent_model);
    // Worktree isolation: build a real git provider only when the spec asks for
    // it and git is available. A failure here leaves the provider unset; the
    // engine then records an honest "ran without isolation" note. The guard
    // outlives `run` (declared before `opts`, borrowed by it).
    let worktree_provider = spec_requests_worktree(&prepared.spec_value)
        .then(worktree::GitWorktreeProvider::new)
        .and_then(Result::ok);
    // Live-progress sink: stamps `.zo/workflows/_active.progress.json` at each
    // topology boundary for the TUI to poll into a workflow tree. Declared before
    // `opts` so it outlives the borrow; a non-project cwd disables writes (no error).
    let progress = progress::LiveProgressSink::new(run_id.clone(), parent_session_id);
    // Phase-3 shadow mode: append every topology event to `<run_id>.events.jsonl`
    // in parallel with the snapshot, so the run timeline is replayable/auditable
    // (`progress::read_event_log`). Tee'd through the engine's single sink seam so
    // the engine signature is unchanged. Both sinks outlive the `opts` borrow.
    let events = progress::EventLogSink::new(run_id);
    let tee = progress::TeeProgressSink::new(vec![&progress, &events]);
    // Verification-command runner for `repeat.until = "command_green"`: runs the
    // command in the main working tree. Declared before `opts` so it outlives
    // the borrow; harmless (never invoked) unless a phase declares command_green.
    let check = |command: &str| {
        run_check_command(command, parent_permission_mode, permission_enforcer)
    };
    // Wire foreground cancellation (BUG-D6): clear any stale signal from a prior
    // run, then point the engine's cancel seam at the process-global flag the TUI
    // sets on Ctrl+C. This stops the phase loop even though the `spawn_blocking`
    // worker it runs on can't be aborted by dropping the turn future.
    engine::clear_foreground_workflow_cancel();
    let mut opts = RunOptions::production().with_cancel(engine::foreground_workflow_cancel_flag());
    if let Some(cache) = file_cache.as_ref() {
        opts = opts.with_cache(cache);
    }
    if let Some(cache) = semantic_cache.as_ref() {
        opts = opts.with_semantic_cache(cache);
    }
    if let Some(provider) = worktree_provider.as_ref() {
        opts = opts.with_worktree(provider);
    }
    opts = opts.with_progress(&tee).with_check(&check);
    let mut backend = LiveBackend {
        parent_model: parent_model.map(str::to_string),
        parent_model_pinned,
        parent_session_id: parent_session_id.map(str::to_string),
        registry,
        // Smuggled by the runtime dispatcher into this call's execution input
        // (`spawn_tool_execution_input`); absent on headless/direct runs.
        tool_call_id: input
            .get("__zo_tool_call_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        mcp_passthrough,
        hook_config: hook_config.clone(),
        parent_permission_mode,
        attempts: std::collections::HashMap::new(),
    };
    run_prepared_workflow_with_backend(&prepared, &mut backend, &opts)
}

/// Cheap pre-parse peek: does the raw spec request `isolation:"worktree"`? Used
/// only to decide whether to stand up a git provider; a malformed spec is still
/// rejected by `validate()` downstream.
fn spec_requests_worktree(spec: &Value) -> bool {
    spec.get("isolation")
        .and_then(Value::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("worktree"))
}

/// Map a finished bash command's `returnCodeInterpretation` to an exit code for
/// `command_green`: `None` = clean exit (0), `Some("exit_code:N")` = N, any
/// other interpretation (e.g. `"timeout"`) = the negative infrastructure
/// sentinel. Pure so the parsing is unit-testable without spawning a shell.
fn interpret_check_exit(interpretation: Option<&str>) -> i32 {
    match interpretation {
        None => 0,
        Some(code) => match code
            .strip_prefix("exit_code:")
            .and_then(|n| n.parse::<i32>().ok())
        {
            Some(126 | 127) | None => engine::CHECK_INFRA_ERROR,
            Some(exit) => exit,
        },
    }
}

/// Production verification runner for `repeat.until = "command_green"`: apply
/// the public `bash` tool's command-intent and user-rule gates, then run in the
/// live process cwd (the main working tree). A denied command, an ask decision,
/// or a missing exit code returns the infrastructure sentinel, never a panic or
/// a green result.
fn run_check_command(
    command: &str,
    parent_permission_mode: Option<PermissionMode>,
    permission_enforcer: Option<&PermissionEnforcer>,
) -> i32 {
    let mode = match parent_permission_mode {
        Some(
            mode @ (PermissionMode::ReadOnly
            | PermissionMode::WorkspaceWrite
            | PermissionMode::DangerFullAccess),
        ) => mode,
        Some(PermissionMode::Prompt | PermissionMode::Allow) => PermissionMode::WorkspaceWrite,
        None => PermissionMode::ReadOnly,
    };
    let mut enforcer = permission_enforcer
        .cloned()
        .unwrap_or_else(|| PermissionEnforcer::new(PermissionPolicy::new(mode)))
        .with_active_mode(mode);
    if mode == PermissionMode::WorkspaceWrite {
        enforcer = enforcer.with_tool_requirement("bash", PermissionMode::WorkspaceWrite);
    }
    if matches!(
        enforcer.check_bash(command),
        EnforcementResult::Denied { .. }
    ) {
        return engine::CHECK_INFRA_ERROR;
    }

    let input_value = json!({ "command": command });
    let Ok(input_json) = serde_json::to_string(&input_value) else {
        return engine::CHECK_INFRA_ERROR;
    };
    if matches!(
        enforcer.check("bash", &input_json),
        EnforcementResult::Denied { .. }
    ) {
        return engine::CHECK_INFRA_ERROR;
    }

    let Ok(input) = serde_json::from_value::<runtime::BashCommandInput>(input_value) else {
        return engine::CHECK_INFRA_ERROR;
    };
    match runtime::execute_bash(input) {
        Ok(output) => interpret_check_exit(output.return_code_interpretation.as_deref()),
        Err(_) => engine::CHECK_INFRA_ERROR,
    }
}

/// Backend-agnostic core, so the parse → validate → run → serialize path is
/// testable offline with a mock backend.
#[cfg(test)]
fn run_workflow_with_backend(
    input: &Value,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
) -> Result<String, ToolError> {
    let prepared = prepare_workflow_input(input)?;
    run_prepared_workflow_with_backend(&prepared, backend, opts)
}

fn run_prepared_workflow_with_backend(
    prepared: &PreparedWorkflowInput,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
) -> Result<String, ToolError> {
    let workflow = WorkflowSpec::from_value(&prepared.spec_value)?.validate()?;
    let report = engine::run(&workflow, &prepared.workflow_input, backend, opts);
    crate::to_pretty_json(report)
}

/// Dry-run entry: parse + validate + normalize the spec, then return a stable
/// structural preview. This deliberately does not invoke the workflow engine,
/// spawn agents, run verification commands, create worktrees, or touch stores.
pub(crate) fn validate_workflow(input: &Value) -> Result<String, ToolError> {
    let prepared = prepare_workflow_input(input)?;
    let workflow = WorkflowSpec::from_value(&prepared.spec_value)?.validate()?;
    crate::to_pretty_json(workflow_preview(&workflow))
}

pub(crate) fn run_workflow_library(input: &Value) -> Result<String, ToolError> {
    library::run(input)
}

pub(crate) fn run_workflow_runs(input: &Value) -> Result<String, ToolError> {
    inspector::run(input)
}

pub(crate) fn run_workflow_skill_project(input: &Value) -> Result<String, ToolError> {
    skill_projection::run(input)
}

pub(super) fn workflow_preview(workflow: &NormalizedWorkflow) -> Value {
    json!({
        "valid": true,
        "name": &workflow.name,
        "description": &workflow.description,
        "mode": workflow_mode_label(workflow.mode),
        "phase_count": workflow.phases.len(),
        "max_agents": workflow.max_agents,
        "max_output_tokens": workflow.max_output_tokens,
        "max_cost_usd": workflow.max_cost_usd,
        "isolation": isolation_label(workflow.isolation),
        "apply": apply_policy_label(workflow.apply),
        "has_synthesize": workflow.synthesize.is_some(),
        "has_judge": workflow.judge.is_some(),
        "phases": workflow
            .phases
            .iter()
            .map(|phase| {
                json!({
                    "id": &phase.id,
                    "source": phase_source_label(&phase.source),
                    "subagent_type": &phase.subagent_type,
                    "model": &phase.model,
                    "has_schema": phase.schema.is_some(),
                    "has_repeat": phase.repeat.is_some(),
                    "has_repair_loop": phase.repair_loop.is_some(),
                })
            })
            .collect::<Vec<_>>()
    })
}

pub(super) fn workflow_mode_label(mode: WorkflowMode) -> &'static str {
    match mode {
        WorkflowMode::Phases => "phases",
        WorkflowMode::Pipeline => "pipeline",
    }
}

fn isolation_label(isolation: Isolation) -> &'static str {
    match isolation {
        Isolation::None => "none",
        Isolation::Worktree => "worktree",
    }
}

fn apply_policy_label(apply: ApplyPolicy) -> &'static str {
    match apply {
        ApplyPolicy::None => "none",
        ApplyPolicy::Sequential => "sequential",
    }
}

pub(super) fn phase_source_label(source: &PhaseSource) -> &'static str {
    match source {
        PhaseSource::Single => "single",
        PhaseSource::Fanout(_) => "fanout",
        PhaseSource::Over(_) => "over",
    }
}

#[derive(Debug)]
struct PreparedWorkflowInput {
    spec_value: Value,
    workflow_input: Value,
}

fn prepare_workflow_input(input: &Value) -> Result<PreparedWorkflowInput, ToolError> {
    prepare_workflow_input_with_loader(input, library::load_spec)
}

fn prepare_workflow_input_with_loader(
    input: &Value,
    load_library_spec: impl Fn(&str) -> Result<Value, ToolError>,
) -> Result<PreparedWorkflowInput, ToolError> {
    if input.get("library").is_some() {
        return expand_library_input(input, load_library_spec);
    }
    if let Some(expansion) = presets::expand_preset(input)? {
        return Ok(PreparedWorkflowInput {
            spec_value: expansion.spec,
            workflow_input: expansion.input,
        });
    }
    let (spec_value, workflow_input) = split_spec_and_input(input);
    Ok(PreparedWorkflowInput {
        spec_value: spec_value.clone(),
        workflow_input,
    })
}

fn expand_library_input(
    input: &Value,
    load_library_spec: impl Fn(&str) -> Result<Value, ToolError>,
) -> Result<PreparedWorkflowInput, ToolError> {
    if input.get("spec").is_some() || input.get("preset").is_some() {
        return Err(ToolError::InvalidInput(
            "`library` is mutually exclusive with `spec` and `preset`".into(),
        ));
    }
    let name = input
        .get("library")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("`library` must be a non-empty string".into()))?;
    Ok(PreparedWorkflowInput {
        spec_value: load_library_spec(name)?,
        workflow_input: input.get("input").cloned().unwrap_or(Value::Null),
    })
}

/// Pick the resume/cache key. An explicit `resumeFromRunId` (harness resume
/// contract) reuses a prior run's phase cache even when the spec or input
/// changed — the edited-script resume: unchanged `agent()` prefixes replay
/// from cache, the first changed call onward runs live. Honored only in the
/// enveloped `{spec, input, resumeFromRunId}` or `{preset, input, resumeFromRunId}`
/// form; in the bare-spec form the key would be part of the spec itself.
/// Default stays the stable (spec, input) hash, which already resumes identical
/// re-runs.
fn resolve_run_id(input: &Value, spec_value: &Value, workflow_input: &Value) -> String {
    input
        .get("resumeFromRunId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| {
            !id.is_empty()
                && (input.get("spec").is_some()
                    || input.get("preset").is_some()
                    || input.get("library").is_some())
        })
        .map_or_else(
            || cache::compute_run_id(spec_value, workflow_input),
            str::to_string,
        )
}

/// Accept both tool-input shapes: `{spec, input}` (a spec plus its runtime
/// argument) or a spec object directly. The presence of a `spec` key
/// disambiguates — a spec never has a top-level `spec` field of its own.
fn split_spec_and_input(input: &Value) -> (&Value, Value) {
    if let Some(spec) = input.get("spec") {
        let workflow_input = input.get("input").cloned().unwrap_or(Value::Null);
        (spec, workflow_input)
    } else {
        (input, Value::Null)
    }
}

/// Production [`AgentBackend`]: launches real sub-agents and waits on the
/// shared completion store — the exact primitives `SpawnMultiAgent` uses.
struct LiveBackend {
    attempts: std::collections::HashMap<String, u64>,
    /// The foreground session model, used as the fallback when a phase/agent and
    /// the role-based default don't pin a model (see `resolve_agent_model`).
    parent_model: Option<String>,
    /// `true` when `parent_model` is an explicit user pin — members without
    /// their own `model` then inherit it instead of smart-routing.
    parent_model_pinned: bool,
    /// Foreground session id stamped onto workflow-spawned agent manifests so
    /// HUD/detail views do not show agents from another chat in the same cwd.
    parent_session_id: Option<String>,
    /// The spawning session's agent registry (t-2511), stamped onto every
    /// member spawn and read by every id-keyed lookup the backend makes.
    registry: Option<std::sync::Arc<crate::misc_tools::AgentRegistry>>,
    /// `tool_use` id of the owning `Workflow` call (the runtime smuggles it
    /// into the execution input), stamped onto every workflow agent's manifest
    /// so the TUI attributes them to this call's transcript batch — mirroring
    /// `parent_session_id` above.
    tool_call_id: Option<String>,
    /// Parent-session MCP passthrough, inherited by every workflow agent so
    /// they advertise and dispatch the session's MCP tools (mirroring
    /// `parent_session_id`/`tool_call_id` above).
    mcp_passthrough: Option<crate::registry::McpPassthrough>,
    /// Parent runtime's hook config, inherited by every workflow agent so
    /// SubagentStart/Stop and tool hooks fire for them exactly like for
    /// `Agent`/`SpawnMultiAgent` spawns (this was the one spawn path that
    /// silently dropped hooks).
    hook_config: runtime::RuntimeHookConfig,
    /// Parent's active permission mode, stamped onto every workflow agent so
    /// the spawn clamp applies to engine-spawned agents exactly like
    /// `Agent`/`SpawnMultiAgent` spawns.
    parent_permission_mode: Option<runtime::PermissionMode>,
}

fn terminal_agent_completions(
    observed: Vec<AgentCompletion>,
) -> (Vec<AgentCompletion>, std::collections::HashSet<String>) {
    let terminal: Vec<AgentCompletion> = observed
        .into_iter()
        .filter(|completion| completion.status != engine::STATUS_STILL_RUNNING)
        .collect();
    let ids = terminal
        .iter()
        .map(|completion| completion.agent_id.clone())
        .collect();
    (terminal, ids)
}

fn startup_watchdog_stops(
    registry: &crate::misc_tools::AgentRegistry,
    pending: &[String],
    extension_used: &mut std::collections::HashSet<String>,
    policy: engine::StartupWatchdogPolicy,
) -> Vec<String> {
    let now_epoch_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let mut stops = Vec::new();
    for id in pending {
        let Some(snapshot) = crate::misc_tools::agent_activity_snapshot_by_id(registry, id) else {
            continue;
        };
        match engine::startup_watchdog_decision(
            &snapshot,
            now_epoch_secs,
            extension_used.contains(id),
            policy,
        ) {
            engine::StartupWatchdogDecision::Continue => {}
            engine::StartupWatchdogDecision::ExtendOnce => {
                extension_used.insert(id.clone());
            }
            engine::StartupWatchdogDecision::Stop => stops.push(id.clone()),
        }
    }
    stops
}

fn phase_inactivity_stops(
    registry: &crate::misc_tools::AgentRegistry,
    pending: &[String],
    phase_started_at: u64,
    inactivity_timeout: Duration,
) -> Vec<String> {
    let now_epoch_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    pending
        .iter()
        .filter(|id| {
            crate::misc_tools::agent_activity_snapshot_by_id(registry, id).is_some_and(|snapshot| {
                engine::phase_inactivity_exceeded(
                    &snapshot,
                    phase_started_at,
                    now_epoch_secs,
                    inactivity_timeout,
                )
            })
        })
        .cloned()
        .collect()
}

fn stop_agents_with_error(
    registry: &crate::misc_tools::AgentRegistry,
    ids: &[String],
    cancel_reason: &str,
    stop_error: &str,
    on_done: &mut dyn FnMut(&AgentCompletion),
    attempts: Option<&std::collections::HashMap<String, u64>>,
) -> Vec<AgentCompletion> {
    ids.iter()
        .map(|id| {
            let result = match attempts {
                Some(attempts) => attempts.get(id).and_then(|generation| {
                    crate::misc_tools::cancel_and_salvage_agent_attempt(
                        registry, id, *generation, cancel_reason,
                    )
                }),
                None => crate::misc_tools::cancel_and_salvage_agent(registry, id, cancel_reason),
            };
            let completion = AgentCompletion {
                agent_id: id.clone(),
                name: id.clone(),
                status: engine::STATUS_STOPPED.to_string(),
                result,
                structured: None,
                error: Some(stop_error.to_string()),
                run: HelperRun::default(),
            };
            on_done(&completion);
            completion
        })
        .collect()
}

impl LiveBackend {
    fn owns_attempt(&self, id: &str) -> bool {
        self.attempts.get(id).is_some_and(|generation| {
            self.registry().manifest_by_id(id)
                .is_some_and(|manifest| manifest.run_generation == *generation)
        })
    }

    fn changed_attempt_completion(id: &str) -> AgentCompletion {
        AgentCompletion {
            agent_id: id.to_string(),
            name: id.to_string(),
            status: engine::STATUS_FAILED.to_string(),
            result: None,
            structured: None,
            error: Some("workflow agent execution generation is unverified or changed".to_string()),
            run: HelperRun::default(),
        }
    }

    /// The registry every id-keyed lookup this backend makes goes through:
    /// the spawning session's, else the documented cwd fallback.
    fn registry(&self) -> std::sync::Arc<crate::misc_tools::AgentRegistry> {
        self.registry
            .clone()
            .unwrap_or_else(crate::misc_tools::AgentRegistry::unowned_from_cwd)
    }

    /// Workflow-only wait loop with independent startup, inactivity, and hard
    /// cap policies. Task progress resets the inactivity window; transport and
    /// reasoning activity do not. The hard cap remains the final safety bound.
    fn wait_with_startup_watchdog(
        registry: &crate::misc_tools::AgentRegistry,
        ids: &[String],
        timeout: Duration,
        on_done: &mut dyn FnMut(&AgentCompletion),
    ) -> Vec<AgentCompletion> {
        let mut watch = wait::WaitState::new(
            std::time::Instant::now(),
            engine::phase_hard_timeout_from_env(timeout),
        );
        Self::wait_with_watch(registry, ids, timeout, &mut watch, on_done, None)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one wait loop owns completion races, startup, inactivity, and the hard cap"
    )]
    fn wait_with_watch(
        registry: &crate::misc_tools::AgentRegistry,
        ids: &[String],
        timeout: Duration,
        watch: &mut wait::WaitState,
        on_done: &mut dyn FnMut(&AgentCompletion),
        attempts: Option<&std::collections::HashMap<String, u64>>,
    ) -> Vec<AgentCompletion> {
        let startup_policy = engine::startup_watchdog_policy_from_env();
        let mut pending = ids.to_vec();
        let mut completions = Vec::with_capacity(ids.len());
        let mut hard_cap_reached = false;

        while !pending.is_empty() {
            let slice = match watch.next_slice(
                std::time::Instant::now(),
                engine::foreground_workflow_cancel_flag().load(std::sync::atomic::Ordering::Relaxed),
            ) {
                Ok(slice) => slice,
                Err(wait::WaitStop::Cancelled) => break,
                Err(wait::WaitStop::Deadline) => {
                    hard_cap_reached = true;
                    break;
                }
            };
            let observed = crate::misc_tools::wait_for_next_agent_completions(
                &pending,
                slice,
                Some(engine::foreground_workflow_cancel_flag()),
                on_done,
            );
            let (observed, finished_ids) = terminal_agent_completions(observed);
            completions.extend(observed);
            pending.retain(|id| !finished_ids.contains(id));
            if pending.is_empty() {
                break;
            }
            if engine::foreground_workflow_cancel_flag()
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                break;
            }

            let mut startup_stops = startup_policy.map_or_else(Vec::new, |policy| {
                startup_watchdog_stops(registry, &pending, &mut watch.startup_extensions, policy)
            });
            // Close the slice-boundary race: an agent may have completed after
            // the observed wait returned `still_running` but before this
            // decision. Re-probe the completion store at zero wait and never
            // downgrade a real terminal result to a watchdog stop.
            let raced = crate::misc_tools::wait_for_agent_completions_cancellable(
                &startup_stops,
                Duration::ZERO,
                Some(engine::foreground_workflow_cancel_flag()),
            );
            let (raced, raced_ids) = terminal_agent_completions(raced);
            for completion in &raced {
                on_done(completion);
            }
            completions.extend(raced);
            startup_stops.retain(|id| !raced_ids.contains(id));
            pending.retain(|id| !raced_ids.contains(id));

            completions.extend(stop_agents_with_error(
                registry,
                &startup_stops,
                engine::STARTUP_NO_PROGRESS_STOP_ERROR,
                engine::STARTUP_NO_PROGRESS_STOP_ERROR,
                on_done,
                attempts,
            ));
            if !startup_stops.is_empty() {
                let stopped: std::collections::HashSet<&str> =
                    startup_stops.iter().map(String::as_str).collect();
                pending.retain(|id| !stopped.contains(id.as_str()));
            }

            let mut inactivity_stops =
                phase_inactivity_stops(registry, &pending, watch.started_at, timeout);
            if !inactivity_stops.is_empty() {
                // Apply the same slice-boundary race guard as startup stops.
                let raced = crate::misc_tools::wait_for_agent_completions_cancellable(
                    &inactivity_stops,
                    Duration::ZERO,
                    Some(engine::foreground_workflow_cancel_flag()),
                );
                let (raced, raced_ids) = terminal_agent_completions(raced);
                for completion in &raced {
                    on_done(completion);
                }
                completions.extend(raced);
                inactivity_stops.retain(|id| !raced_ids.contains(id));
                pending.retain(|id| !raced_ids.contains(id));

                completions.extend(stop_agents_with_error(
                    registry,
                    &inactivity_stops,
                    engine::PHASE_TIMEOUT_STOP_ERROR,
                    engine::PHASE_TIMEOUT_STOP_ERROR,
                    on_done,
                    attempts,
                ));
                let stopped: std::collections::HashSet<&str> =
                    inactivity_stops.iter().map(String::as_str).collect();
                pending.retain(|id| !stopped.contains(id.as_str()));
            }
            if attempts.is_some() {
                break;
            }
        }

        if hard_cap_reached && !pending.is_empty() {
            let hard_capped = std::mem::take(&mut pending);
            completions.extend(stop_agents_with_error(
                registry,
                &hard_capped,
                engine::PHASE_HARD_TIMEOUT_STOP_ERROR,
                engine::PHASE_HARD_TIMEOUT_STOP_ERROR,
                on_done,
                attempts,
            ));
        }

        completions.extend(pending.into_iter().map(|id| AgentCompletion {
            agent_id: id,
            name: String::new(),
            status: engine::STATUS_STILL_RUNNING.to_string(),
            result: None,
            structured: None,
            error: None,
            run: HelperRun::default(),
        }));
        completions
    }
}

fn apply_smart_model_to_workflow_agent(
    parent_model: Option<&str>,
    parent_model_pinned: bool,
    input: &mut AgentInput,
) {
    // A bounded startup-recovery reroute is already a trusted Smart decision:
    // preserve its explicitly selected alternate instead of recomputing back
    // onto the provider that just stalled twice.
    if input
        .route_model
        .as_deref()
        .is_some_and(|model| !model.trim().is_empty())
    {
        return;
    }
    if input.model.as_deref().is_some_and(|model| !model.trim().is_empty()) {
        return;
    }
    // An agent whose own model was chosen explicitly for it passes that model
    // to its workflow members verbatim. A foreground `/model` pin does NOT set
    // this (it binds the main turn only), so a `sol` session's workflow members
    // still route per role instead of all running on `sol`.
    if parent_model_pinned {
        return;
    }
    if let Some(choice) = crate::misc_tools::smart_parent_model_for_agent(parent_model, input) {
        input.route_model = choice.model;
        input.route_reason = choice.reason;
        input.route_fallback_models = choice.fallback_models;
        input.route_effort = choice.effort;
        input.route_role = Some(choice.decision_meta.role);
        input.route_complexity = Some(choice.decision_meta.complexity);
        input.route_risk = Some(choice.decision_meta.risk);
        input.route_source = Some(choice.decision_meta.route_source);
    }
}

impl AgentBackend for LiveBackend {
    fn spawn(&mut self, mut input: AgentInput) -> Result<String, ToolError> {
        input.parent_session_id.clone_from(&self.parent_session_id);
        input.registry.clone_from(&self.registry);
        input.parent_permission_mode = self.parent_permission_mode;
        input.tool_call_id.clone_from(&self.tool_call_id);
        input.mcp_passthrough.clone_from(&self.mcp_passthrough);
        apply_smart_model_to_workflow_agent(
            self.parent_model.as_deref(),
            self.parent_model_pinned,
            &mut input,
        );
        // Workflow agents don't inherit the parent LSP (out of scope for the
        // debugger-evidence wiring, and they are frequently worktree-isolated).
        crate::misc_tools::execute_agent_with_parent_model_and_hooks(
            input,
            self.parent_model.as_deref(),
            None,
            Some(&self.hook_config),
        )
        .map(|output| {
            self.attempts.insert(output.agent_id.clone(), output.run_generation);
            output.agent_id
        })
    }

    fn allows_readonly_refill(&self) -> bool {
        self.parent_permission_mode == Some(runtime::PermissionMode::ReadOnly)
    }

    fn worker_is_live(&self, id: &str) -> bool {
        self.attempts.get(id).is_some_and(|generation| {
            crate::misc_tools::agent_worker_generation_is_live(id, *generation)
        })
    }

    fn wait_next(
        &self,
        ids: &[String],
        timeout: Duration,
        watch: &mut wait::WaitState,
    ) -> Vec<AgentCompletion> {
        let registry = self.registry();
        let pending: Vec<String> = ids.iter().filter(|id| self.owns_attempt(id)).cloned().collect();
        let mut ignore = |_completion: &AgentCompletion| {};
        let mut completions = Self::wait_with_watch(
            &registry, &pending, timeout, watch, &mut ignore, Some(&self.attempts),
        );
        completions.extend(ids.iter().filter(|id| !pending.contains(id)).map(|id| {
            Self::changed_attempt_completion(id)
        }));
        for completion in &mut completions {
            if !self.owns_attempt(&completion.agent_id) {
                *completion = Self::changed_attempt_completion(&completion.agent_id);
            }
        }
        if completions.iter().any(|completion| {
            completion.status != engine::STATUS_STILL_RUNNING
                && self.worker_is_live(&completion.agent_id)
        }) {
            // Completion publication precedes the worker registration guard's drop.
            if let Ok(slice) = watch.next_slice(std::time::Instant::now(), false) {
                std::thread::sleep(slice.min(Duration::from_millis(100)));
            }
        }
        completions
    }

    fn usage_cost(&self, agent_id: &str) -> Option<crate::misc_tools::AgentUsageCost> {
        crate::misc_tools::agent_usage_cost_for_attempt(
            &self.registry(),
            agent_id,
            *self.attempts.get(agent_id)?,
        )
    }

    fn wait(&self, ids: &[String], timeout: Duration) -> Vec<AgentCompletion> {
        let mut ignore = |_completion: &AgentCompletion| {};
        Self::wait_with_startup_watchdog(&self.registry(), ids, timeout, &mut ignore)
    }

    fn agent_registry(&self) -> Option<std::sync::Arc<crate::misc_tools::AgentRegistry>> {
        self.registry.clone()
    }

    fn wait_observed(
        &self,
        ids: &[String],
        timeout: Duration,
        on_done: &mut dyn FnMut(&AgentCompletion),
    ) -> Vec<AgentCompletion> {
        // True streaming observer: each agent's completion fires mid-barrier in
        // completion order, so the live progress doc moves per agent instead of
        // jumping 0→100% at the phase boundary.
        Self::wait_with_startup_watchdog(&self.registry(), ids, timeout, on_done)
    }

    fn cancel(&self, id: &str) -> Option<AgentCompletion> {
        let partial = if self.allows_readonly_refill() {
            crate::misc_tools::cancel_and_salvage_agent_attempt(
                &self.registry(), id, *self.attempts.get(id)?,
                "agent exceeded workflow phase timeout",
            )
        } else {
            crate::misc_tools::cancel_and_salvage_agent(
                &self.registry(), id, "agent exceeded workflow phase timeout",
            )
        };
        Some(AgentCompletion {
            agent_id: id.to_string(),
            name: id.to_string(),
            status: "stopped".to_string(),
            result: partial,
            structured: None,
            // The exact marker the engine's timeout retry pass keys on.
            error: Some(engine::PHASE_TIMEOUT_STOP_ERROR.to_string()),
            run: HelperRun::default(),
        })
    }

    fn activity(&self, id: &str) -> Option<crate::misc_tools::AgentActivitySnapshot> {
        let mut activity = crate::misc_tools::agent_activity_snapshot_by_id(&self.registry(), id)?;
        if self.parent_model_pinned {
            activity.fallback_models.clear();
        }
        Some(activity)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.previous.take() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    /// Minimal echo backend: every spawn completes with `result-<n>`.
    #[derive(Default)]
    struct EchoBackend {
        next: usize,
        completions: HashMap<String, AgentCompletion>,
        models: Vec<Option<String>>,
        subagent_types: Vec<Option<String>>,
        prompts: Vec<String>,
    }

    impl AgentBackend for EchoBackend {
        fn spawn(&mut self, input: AgentInput) -> Result<String, ToolError> {
            self.models.push(input.model.clone());
            self.subagent_types.push(input.subagent_type.clone());
            self.prompts.push(input.prompt.clone());
            let id = format!("echo-{}", self.next);
            self.next += 1;
            self.completions.insert(
                id.clone(),
                AgentCompletion {
                    agent_id: id.clone(),
                    name: id.clone(),
                    status: "completed".to_string(),
                    // Echo the prompt so tests can assert token substitution.
                    result: Some(input.prompt),
                    structured: None,
                    error: None,
                    run: HelperRun::default(),
                },
            );
            Ok(id)
        }

        fn wait(&self, ids: &[String], _timeout: Duration) -> Vec<AgentCompletion> {
            ids.iter()
                .filter_map(|id| self.completions.get(id).cloned())
                .collect()
        }
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

    #[test]
    fn live_barrier_leaves_silent_running_tool_to_hard_cap() {
        let _env_lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let store = std::env::temp_dir().join(format!(
            "zo-workflow-active-tool-{unique}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&store).expect("create agent store");
        let _store = EnvVarGuard::set("ZO_AGENT_STORE", &store);
        let _startup = EnvVarGuard::set("ZO_WORKFLOW_STARTUP_WATCHDOG", "off");
        let _hard_cap = EnvVarGuard::set("ZO_WORKFLOW_PHASE_HARD_TIMEOUT_SECS", "2");
        engine::clear_foreground_workflow_cancel();

        let agent_id = format!("active-tool-{unique}");
        let manifest_path = store.join(format!("{agent_id}.json"));
        let output_path = store.join(format!("{agent_id}.md"));
        std::fs::write(&output_path, "# Agent\n").expect("write agent output");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        std::fs::write(
            &manifest_path,
            serde_json::to_vec(&json!({
                "agentId": agent_id,
                "name": "active tool",
                "description": "exercise the live workflow barrier",
                "subagentType": "general-purpose",
                "model": "gpt-5.6-sol",
                "status": "running",
                "outputFile": output_path,
                "manifestFile": manifest_path,
                "createdAt": now.to_string(),
                "startedAt": now.to_string(),
                "currentTool": "bash",
                "activity": {
                    "firstTaskActionAt": now,
                    "lastTaskProgressAt": now
                }
            }))
            .expect("serialize manifest"),
        )
        .expect("write manifest");

        let started = std::time::Instant::now();
        let completions = LiveBackend::wait_with_startup_watchdog(
            &crate::misc_tools::AgentRegistry::unowned_from_cwd(),
            std::slice::from_ref(&agent_id),
            Duration::from_secs(1),
            &mut |_| {},
        );

        assert!(started.elapsed() >= Duration::from_secs(2));
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].status, engine::STATUS_STOPPED);
        assert_eq!(
            completions[0].error.as_deref(),
            Some(engine::PHASE_HARD_TIMEOUT_STOP_ERROR),
            "an active tool must never be stopped as task inactivity"
        );
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn workflow_check_runner_enforces_parent_permission_before_execution() {
        // The allowed cases spawn a real shell, which resolves
        // `env::current_dir()` and `PATH`; serialize against the tests that
        // mutate them (all of which hold `env_lock`) so a concurrent
        // `set_current_dir` cannot invalidate the cwd mid-spawn.
        let _env_lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            run_check_command("true", Some(PermissionMode::ReadOnly), None),
            0,
            "a provably read-only check should run"
        );
        for command in [
            "sh -c 'exit 0'",
            "awk 'BEGIN { exit 0 }'",
            "find . -maxdepth 0 -fprint /tmp/zo-workflow-check",
        ] {
            assert_eq!(
                run_check_command(command, Some(PermissionMode::ReadOnly), None),
                engine::CHECK_INFRA_ERROR,
                "a denied check must never execute or appear green: {command:?}"
            );
        }
        assert_eq!(
            run_check_command("true", Some(PermissionMode::Prompt), None),
            0,
            "an already-approved workflow may still run a read-only check"
        );
        for mode in [PermissionMode::Prompt, PermissionMode::Allow] {
            assert_eq!(
                run_check_command("true > /dev/null", Some(mode), None),
                0,
                "an approved workflow must permit workspace-write checks in {mode:?}"
            );
        }
        assert_eq!(
            run_check_command("true", None, None),
            0,
            "missing parent metadata must retain safe read-only checks"
        );
    }

    #[test]
    fn workflow_check_runner_enforces_deny_and_ask_rules() {
        use runtime::RuntimePermissionRuleConfig;

        for (label, rules) in [
            (
                "deny",
                RuntimePermissionRuleConfig::new(
                    Vec::new(),
                    vec!["bash(true)".to_string()],
                    Vec::new(),
                ),
            ),
            (
                "ask",
                RuntimePermissionRuleConfig::new(
                    Vec::new(),
                    Vec::new(),
                    vec!["bash(true)".to_string()],
                ),
            ),
        ] {
            let enforcer = PermissionEnforcer::new(
                PermissionPolicy::new(PermissionMode::Prompt)
                    .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
                    .with_permission_rules(&rules),
            );
            assert_eq!(
                run_check_command("true", Some(PermissionMode::Prompt), Some(&enforcer)),
                engine::CHECK_INFRA_ERROR,
                "a workflow check subject to a user {label} rule must fail closed"
            );
        }
    }

    #[test]
    fn interpret_check_exit_maps_return_code_interpretation() {
        // Mirrors bash.rs: None on clean exit, Some("exit_code:N") otherwise.
        assert_eq!(interpret_check_exit(None), 0);
        assert_eq!(interpret_check_exit(Some("exit_code:1")), 1);
        assert_eq!(interpret_check_exit(Some("exit_code:101")), 101);
        for unavailable in ["exit_code:126", "exit_code:127"] {
            assert_eq!(
                interpret_check_exit(Some(unavailable)),
                engine::CHECK_INFRA_ERROR,
                "an unavailable verification command is not implementation evidence"
            );
        }
        // A non-exit_code interpretation (e.g. timeout) is infrastructure red,
        // not evidence that the implementation itself failed verification.
        assert_eq!(
            interpret_check_exit(Some("timeout")),
            engine::CHECK_INFRA_ERROR
        );
    }

    #[test]
    fn resolve_run_id_prefers_explicit_resume_id_in_enveloped_form() {
        let spec = json!({ "name": "wf", "phases": [{ "id": "a", "prompt": "p" }] });
        let enveloped = json!({ "spec": spec, "input": "x", "resumeFromRunId": " wf-prior " });
        let (spec_value, workflow_input) = split_spec_and_input(&enveloped);
        assert_eq!(
            resolve_run_id(&enveloped, spec_value, &workflow_input),
            "wf-prior",
            "explicit resume id wins (trimmed) so an edited spec replays the prior cache"
        );

        // Bare-spec form: a stray `resumeFromRunId` would be part of the spec
        // itself, so it must NOT hijack the cache key.
        let bare = json!({
            "name": "wf",
            "phases": [{ "id": "a", "prompt": "p" }],
            "resumeFromRunId": "wf-prior"
        });
        let (spec_value, workflow_input) = split_spec_and_input(&bare);
        assert_eq!(
            resolve_run_id(&bare, spec_value, &workflow_input),
            cache::compute_run_id(spec_value, &workflow_input),
            "bare-spec form falls back to the stable (spec, input) hash"
        );

        // Blank/whitespace ids are ignored — fall back to the stable hash.
        let blank = json!({ "spec": { "name": "wf", "phases": [] }, "resumeFromRunId": "  " });
        let (spec_value, workflow_input) = split_spec_and_input(&blank);
        assert_eq!(
            resolve_run_id(&blank, spec_value, &workflow_input),
            cache::compute_run_id(spec_value, &workflow_input)
        );
    }

    #[test]
    fn workflow_smart_resolver_does_not_override_explicit_model() {
        let mut input = AgentInput {
            route_probe_confidence: None,
            fork_source: None,
            allow_cross_provider: false,
            description: "verify".to_string(),
            prompt: "run tests".to_string(),
            subagent_type: Some("Verification".to_string()),
            name: None,
            addressable_name: None,
            model: Some("User/Explicit".to_string()),
            cwd: None,
            schema: None,
            background: Some(false),
            workflow_member: true,
            one_shot: false,
            plan_shape: None,
            route_tax: None,
            api_concurrency: None,
            parent_session_id: None,
            registry: None,
            tool_call_id: None,
            mcp_passthrough: None,
            parent_permission_mode: None,
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
        apply_smart_model_to_workflow_agent(Some("claude-sonnet-main"), false, &mut input);
        assert_eq!(input.model.as_deref(), Some("User/Explicit"));

        input.model = None;
        input.route_model = Some("claude-sonnet-4-6".to_string());
        input.route_source = Some("fallback".to_string());
        apply_smart_model_to_workflow_agent(Some("gpt-5.6-sol"), false, &mut input);
        assert_eq!(
            input.route_model.as_deref(),
            Some("claude-sonnet-4-6"),
            "a trusted startup fallback must not be smart-routed back to the stalled provider"
        );
        assert_eq!(input.route_source.as_deref(), Some("fallback"));
    }

    #[test]
    // One env-scoped scenario (config home + custom provider + a full
    // `AgentInput` literal); splitting it would move the env guard away from
    // the assertions it protects.
    #[allow(clippy::too_many_lines)]
    fn workflow_smart_resolver_sets_route_model_and_reason() {
        use std::sync::PoisonError;
        use std::time::{SystemTime, UNIX_EPOCH};

        // Scope ZO_CONFIG_HOME / custom providers under the crate-wide env lock
        // (the same lock the smart-router env tests use) so this config-home
        // override never lands mid-write in another module's env-scoped test.
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let config_home = std::env::temp_dir().join(format!(
            "zo-workflow-route-model-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&config_home).expect("config dir");
        std::fs::write(
            config_home.join("settings.json"),
            serde_json::to_string_pretty(&json!({
                "smart": {"enabled": true},
                "modelRouter": {
                    "subagents": {
                        "Verification": {"mode": "pinned", "model": "Subagent/Model-Z"}
                    }
                }
            }))
            .expect("json"),
        )
        .expect("write settings");

        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        let prior_zo_home = std::env::var_os("ZO_HOME");
        let prior_custom_providers = std::env::var_os(api::CUSTOM_PROVIDERS_ENV);
        std::env::set_var("ZO_CONFIG_HOME", &config_home);
        std::env::remove_var("ZO_HOME");
        std::env::set_var(
            api::CUSTOM_PROVIDERS_ENV,
            r#"[{"name":"Verifier","base_url":"http://verifier.local/v1","models":["Subagent/Model-Z"],"requires_auth":false}]"#,
        );
        api::refresh_custom_providers_from_env();

        let mut input = AgentInput {
            route_probe_confidence: None,
            fork_source: None,
            allow_cross_provider: false,
            description: "verify the code".to_string(),
            prompt: "run verification".to_string(),
            subagent_type: Some("Verification".to_string()),
            name: None,
            addressable_name: None,
            model: None,
            cwd: None,
            schema: None,
            background: Some(false),
            workflow_member: true,
            one_shot: false,
            plan_shape: None,
            route_tax: None,
            api_concurrency: None,
            parent_session_id: None,
            registry: None,
            tool_call_id: None,
            mcp_passthrough: None,
            parent_permission_mode: None,
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
        apply_smart_model_to_workflow_agent(Some("claude-sonnet-main"), false, &mut input);

        // The bug: only `route_reason` was set, so the routed model was silently
        // dropped and the workflow agent ran on the parent model. Both must be set.
        assert_eq!(
            input.route_model.as_deref(),
            Some("Subagent/Model-Z"),
            "the resolved smart-route model must be applied so the workflow agent actually runs on it",
        );
        assert!(
            input.route_reason.as_deref().is_some_and(|reason| reason.contains("pin")),
            "the human-readable route reason is still stamped: {:?}",
            input.route_reason,
        );

        // Same settings, but the parent session model is a user pin: the
        // member must inherit it verbatim — no smart route at all (the
        // "explicit sol session ran its implement agent on terra" downgrade).
        input.route_model = None;
        input.route_reason = None;
        input.route_source = None;
        apply_smart_model_to_workflow_agent(Some("claude-sonnet-main"), true, &mut input);
        assert_eq!(
            input.route_model, None,
            "a pinned parent model must suppress smart routing for workflow members",
        );
        assert_eq!(input.route_reason, None);
        assert_eq!(input.model, None, "member inherits the parent model via the spawn fallback");

        match prior_config_home {
            Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
            None => std::env::remove_var("ZO_CONFIG_HOME"),
        }
        match prior_zo_home {
            Some(value) => std::env::set_var("ZO_HOME", value),
            None => std::env::remove_var("ZO_HOME"),
        }
        match prior_custom_providers {
            Some(value) => std::env::set_var(api::CUSTOM_PROVIDERS_ENV, value),
            None => std::env::remove_var(api::CUSTOM_PROVIDERS_ENV),
        }
        api::refresh_custom_providers_from_env();
        let _ = std::fs::remove_dir_all(&config_home);
    }

    fn readonly_backend(registry: std::sync::Arc<crate::misc_tools::AgentRegistry>) -> LiveBackend {
        LiveBackend {
            attempts: HashMap::new(), parent_model: None, parent_model_pinned: false,
            parent_session_id: None, registry: Some(registry), tool_call_id: None,
            mcp_passthrough: None, hook_config: runtime::RuntimeHookConfig::default(),
            parent_permission_mode: Some(PermissionMode::ReadOnly),
        }
    }

    #[test]
    fn readonly_refill_live_permission_gate_is_fail_closed() {
        let registry = crate::misc_tools::AgentRegistry::unowned_from_cwd();
        let mut backend = readonly_backend(registry);
        for mode in [None, Some(PermissionMode::WorkspaceWrite), Some(PermissionMode::DangerFullAccess), Some(PermissionMode::ReadOnly)] {
            backend.parent_permission_mode = mode;
            assert_eq!(backend.allows_readonly_refill(), mode == Some(PermissionMode::ReadOnly));
        }
    }

    #[test]
    fn readonly_refill_live_rejects_a_replaced_execution_generation() {
        // `into_inner`, like every other holder of this lock: a poisoned env
        // lock is a SIBLING test's panic, not this test's fault. The bare
        // `unwrap()` here turned one flake into two on the 2026-09-14 lane —
        // `reclaim_extension…` panicked holding the lock, and this test died
        // at `PoisonError` before running a line of its own.
        let _env_lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let store = tempfile::tempdir().unwrap();
        let _store = EnvVarGuard::set("ZO_AGENT_STORE", store.path());
        let id = "readonly-replaced-generation";
        let path = store.path().join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_vec(&json!({
            "agentId": id, "name": id, "description": "fixture", "status": "running",
            "subagentType": "general-purpose", "model": "fixture",
            "runGeneration": 2, "outputFile": store.path().join(format!("{id}.md")),
            "manifestFile": path, "createdAt": "1", "startedAt": "1"
        })).unwrap()).unwrap();
        std::fs::write(store.path().join(format!("{id}.md")), "").unwrap();
        let mut backend = readonly_backend(crate::misc_tools::AgentRegistry::unowned_from_cwd());
        assert_eq!(backend.registry().manifest_by_id(id).unwrap().run_generation, 2);
        backend.attempts.insert(id.to_string(), 1);
        let mut watch = wait::WaitState::new(std::time::Instant::now(), Duration::from_secs(1));
        let result = backend.wait_next(&[id.to_string()], Duration::from_secs(1), &mut watch);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, engine::STATUS_FAILED);
        assert!(result[0].result.is_none());
        assert!(result[0].error.as_ref().unwrap().contains("generation"));
        assert!(backend.usage_cost(id).is_none());
        backend.cancel(id);
        let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["runGeneration"], 2);
        assert_eq!(saved["status"], "running", "old workflow cancelled the replacement");
    }

    #[test]
    fn live_backend_cancel_returns_stopped_completion() {
        let backend = LiveBackend {
            parent_model_pinned: false,
            parent_model: None,
            parent_session_id: None,
            registry: None,
            tool_call_id: None,
            mcp_passthrough: None,
            hook_config: runtime::RuntimeHookConfig::default(),
            parent_permission_mode: None,
            attempts: std::collections::HashMap::new(),
        };

        let completion = backend
            .cancel("missing-agent")
            .expect("live backend should override cancel");

        assert_eq!(completion.agent_id, "missing-agent");
        assert_eq!(completion.status, "stopped");
        assert!(completion.error.unwrap().contains("workflow phase timeout"));
    }

    #[test]
    fn tool_spec_is_registered_with_expected_permissions() {
        let specs = tool_specs();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Workflow");
        // Spawn-family tools are unprivileged since the child-mode clamp:
        // workflow agents inherit (at most) the parent session's mode.
        assert_eq!(specs[0].required_permission, PermissionMode::ReadOnly);
        assert_eq!(specs[1].name, "WorkflowValidate");
        assert_eq!(specs[1].required_permission, PermissionMode::ReadOnly);
        assert_eq!(specs[2].name, "WorkflowLibrary");
        assert_eq!(specs[2].required_permission, PermissionMode::WorkspaceWrite);
        assert_eq!(specs[3].name, "WorkflowRuns");
        assert_eq!(specs[3].required_permission, PermissionMode::ReadOnly);
        assert_eq!(specs[4].name, "WorkflowSkillProject");
        assert_eq!(specs[4].required_permission, PermissionMode::WorkspaceWrite);
    }

    /// Proportionality contract (the live over-orchestration incidents: a
    /// 25-agent workflow + readability repair loop for ONE markdown brief).
    /// The when-NOT-to-use criteria live in the tool description itself —
    /// CC-style — so every surface that sees the tool sees the bar, and the
    /// model applies it per ask without a host-side difficulty classifier.
    #[test]
    fn workflow_description_reserves_workflows_for_real_scale() {
        let description = tool_specs().remove(0).description;
        assert!(description.contains("only for dependent multi-phase work at real scale"));
        assert!(description.contains("never for a bounded fix, a routine question, or a document"));
        assert!(description.contains("at most one review pass"));
        assert!(description.contains("not a workflow or a repair loop"));
        assert!(description.contains("do not add a standalone analysis phase"));
        assert!(description.contains("Do not set `synthesize` for a single implement→verify chain"));
        assert!(description.contains("run the requested comprehensive suite once"));
        assert!(description.contains("only after a fix or an inconclusive/unstable result"));
    }

    #[test]
    fn input_schema_requires_a_prompt_on_every_phase() {
        // Guards the harness fix for the "phase `<id>` is missing a `prompt`"
        // failure: the per-phase schema must structurally mark `id` + `prompt`
        // required, in both the direct (`phases`) and wrapped (`spec.phases`)
        // forms, so the model never emits a promptless phase. A regression back
        // to the old freeform `additionalProperties:true` schema fails here.
        let schema = tool_specs().remove(0).input_schema;
        let props = &schema["properties"];

        let direct = &props["phases"]["items"]["required"];
        assert_eq!(direct, &json!(["id", "prompt"]), "direct form");

        let wrapped = &props["spec"]["properties"]["phases"]["items"]["required"];
        assert_eq!(wrapped, &json!(["id", "prompt"]), "wrapped {{spec}} form");
    }

    #[test]
    fn input_schema_exposes_cross_model_preset() {
        let schema = tool_specs().remove(0).input_schema;
        let props = &schema["properties"];
        assert_eq!(
            props["preset"]["enum"],
            json!(["cross_model_verified", "gpt_claude_verified"])
        );
        assert_eq!(props["verify_command"]["type"], "string");
        assert_eq!(props["coding_model"]["type"], "string");
        assert_eq!(props["review_model"]["type"], "string");
        assert_eq!(props["design_model"]["type"], "string");
        assert_eq!(props["repair_model"]["type"], "string");
        assert_eq!(props["design"]["type"], "boolean");
        assert_eq!(
            props["phases"]["items"]["properties"]["model"]["type"],
            "string"
        );
        assert_eq!(props["max_rounds"]["minimum"], 1);
    }

    #[test]
    fn preset_form_expands_and_runs_through_standard_engine() {
        let input = json!({
            "preset": "cross_model_verified",
            "input": "make the smallest safe parser fix",
            "verify_command": "cargo test -p tools",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "max_rounds": 1
        });
        let mut backend = EchoBackend::default();
        let opts = RunOptions {
            check: Some(&|command| i32::from(command != "cargo test -p tools")),
            ..fast_opts()
        };

        let output = run_workflow_with_backend(&input, &mut backend, &opts).expect("workflow runs");
        let report: Value = serde_json::from_str(&output).expect("json report");
        let phases = report["phases"].as_array().expect("phases");
        assert_eq!(report["name"], "cross-model-verified");
        assert_eq!(phases.len(), 5);
        assert_eq!(phases[0]["id"], "preflight");
        assert_eq!(phases[3]["id"], "repair_until_green");
        assert_eq!(
            backend.models,
            vec![
                Some("claude-opus-4-8".to_string()),
                Some("gpt-5.5-fast".to_string()),
                Some("claude-opus-4-8".to_string()),
                Some("gpt-5.5-fast".to_string()),
                Some("claude-opus-4-8".to_string()),
                Some("claude-opus-4-8".to_string()),
            ],
            "preset must pin distinct models across implementation and review/verification phases"
        );
    }

    #[test]
    fn workflow_validate_direct_spec_returns_preview_without_execution_fields() {
        let input = json!({
            "name": "demo",
            "description": "preview only",
            "mode": "phases",
            "budget": { "max_agents": 5 },
            "isolation": "none",
            "apply": "none",
            "phases": [
                { "id": "p", "prompt": "do it" },
                { "id": "review", "over": "p", "prompt": "review {item}", "schema": { "type": "object" } }
            ]
        });

        let raw = validate_workflow(&input).expect("validates");
        let preview: Value = serde_json::from_str(&raw).expect("preview JSON");
        assert_eq!(preview["valid"], true);
        assert_eq!(preview["name"], "demo");
        assert_eq!(preview["description"], "preview only");
        assert_eq!(preview["mode"], "phases");
        assert_eq!(preview["phase_count"], 2);
        assert_eq!(preview["max_agents"], 5);
        assert_eq!(preview["max_output_tokens"], Value::Null);
        assert_eq!(preview["max_cost_usd"], Value::Null);
        assert_eq!(preview["isolation"], "none");
        assert_eq!(preview["apply"], "none");
        assert_eq!(preview["has_synthesize"], false);
        assert_eq!(preview["has_judge"], false);
        assert_eq!(preview["phases"][0]["id"], "p");
        assert_eq!(preview["phases"][0]["source"], "single");
        assert_eq!(preview["phases"][0]["subagent_type"], Value::Null);
        assert_eq!(preview["phases"][0]["model"], Value::Null);
        assert_eq!(preview["phases"][0]["has_schema"], false);
        assert_eq!(preview["phases"][0]["has_repeat"], false);
        assert_eq!(preview["phases"][0]["has_repair_loop"], false);
        assert_eq!(preview["phases"][1]["source"], "over");
        assert_eq!(preview["phases"][1]["has_schema"], true);
        assert!(preview.get("agents_spawned").is_none());
        assert!(preview.get("status").is_none());
    }

    #[test]
    fn workflow_validate_invalid_spec_returns_invalid_input() {
        let input = json!({ "name": "", "phases": [] });
        let err = validate_workflow(&input).expect_err("invalid spec");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        assert!(err.to_string().contains("workflow `name` must not be empty"));
    }

    #[test]
    fn workflow_validate_library_form_returns_preview_and_rejects_mutual_exclusion() {
        let root = {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            std::env::temp_dir().join(format!(
                "zo-workflow-library-expansion-{}-{unique}",
                std::process::id()
            ))
        };
        std::fs::create_dir_all(&root).expect("create temp root");
        let spec = json!({
            "name": "stored",
            "description": "Stored spec",
            "phases": [{ "id": "one", "prompt": "Do {input}" }]
        });
        library::save_at_for_test(&root, "stored_flow", spec, false).expect("save library spec");

        let prepared = prepare_workflow_input_with_loader(
            &json!({ "library": "stored_flow", "input": "task" }),
            |name| library::load_spec_at_for_test(&root, name),
        )
        .expect("library expands");
        assert_eq!(prepared.workflow_input, json!("task"));
        let workflow = WorkflowSpec::from_value(&prepared.spec_value)
            .and_then(WorkflowSpec::validate)
            .expect("saved spec validates");
        let preview = workflow_preview(&workflow);
        assert_eq!(preview["valid"], true);
        assert_eq!(preview["name"], "stored");
        assert_eq!(preview["phase_count"], 1);

        let err = prepare_workflow_input_with_loader(
            &json!({ "library": "stored_flow", "spec": { "name": "x" } }),
            |name| library::load_spec_at_for_test(&root, name),
        )
        .expect_err("library and spec are mutually exclusive");
        assert!(err.to_string().contains("mutually exclusive"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The `{library}` form must reach the actual engine, not just the
    /// validate preview: expansion through the loader and execution through
    /// `run_prepared_workflow_with_backend` is exactly what `run_workflow`
    /// does for a stored spec.
    #[test]
    fn library_form_runs_through_standard_engine() {
        let root = {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            std::env::temp_dir().join(format!(
                "zo-workflow-library-run-{}-{unique}",
                std::process::id()
            ))
        };
        std::fs::create_dir_all(&root).expect("create temp root");
        let spec = json!({
            "name": "stored-run",
            "description": "Stored runnable spec",
            "phases": [{ "id": "only", "prompt": "Do {input}" }]
        });
        library::save_at_for_test(&root, "stored_run", spec, false).expect("save library spec");

        let prepared = prepare_workflow_input_with_loader(
            &json!({ "library": "stored_run", "input": "the task" }),
            |name| library::load_spec_at_for_test(&root, name),
        )
        .expect("library expands");
        let mut backend = EchoBackend::default();
        let raw = run_prepared_workflow_with_backend(&prepared, &mut backend, &fast_opts())
            .expect("stored spec runs");
        let report: Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(report["name"], "stored-run");
        assert_eq!(report["status"], "completed");
        assert_eq!(report["agents_spawned"], 1);
        assert!(
            backend.prompts.iter().any(|prompt| prompt.contains("the task")),
            "workflow input must flow into the stored spec's phase prompt: {:?}",
            backend.prompts
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workflow_validate_preset_form_expands_without_running_engine() {
        let input = json!({
            "preset": "cross_model_verified",
            "input": "make the smallest safe parser fix",
            "verify_command": "cargo test -p tools",
            "coding_model": "gpt-5.5-fast",
            "review_model": "claude-opus-4-8",
            "max_rounds": 1
        });

        let raw = validate_workflow(&input).expect("preset validates");
        let preview: Value = serde_json::from_str(&raw).expect("preview JSON");
        assert_eq!(preview["valid"], true);
        assert_eq!(preview["name"], "cross-model-verified");
        assert_eq!(preview["mode"], "phases");
        assert_eq!(preview["phase_count"], 5);
        assert_eq!(preview["has_synthesize"], true);
        assert_eq!(preview["phases"][0]["id"], "preflight");
        assert_eq!(preview["phases"][0]["source"], "single");
        assert_eq!(preview["phases"][0]["model"], "claude-opus-4-8");
        assert_eq!(preview["phases"][1]["id"], "implement");
        assert_eq!(preview["phases"][1]["source"], "over");
        assert_eq!(preview["phases"][1]["model"], "gpt-5.5-fast");
        assert_eq!(preview["phases"][3]["id"], "repair_until_green");
        assert_eq!(preview["phases"][3]["has_repeat"], true);
        assert!(preview.get("agents_spawned").is_none());
        assert!(preview.get("status").is_none());
    }

    #[test]
    fn direct_spec_form_runs_and_serializes() {
        let input = json!({
            "name": "demo",
            "phases": [{ "id": "only", "prompt": "do it" }]
        });
        let mut backend = EchoBackend::default();
        let raw = run_workflow_with_backend(&input, &mut backend, &fast_opts()).expect("runs");
        let report: Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(report["name"], "demo");
        assert_eq!(report["status"], "completed");
        assert_eq!(report["agents_spawned"], 1);
    }

    #[test]
    fn spec_and_input_form_substitutes_input_token() {
        let input = json!({
            "spec": {
                "name": "demo",
                "phases": [{ "id": "p", "prompt": "task: {input}" }]
            },
            "input": "ship it"
        });
        let mut backend = EchoBackend::default();
        let raw = run_workflow_with_backend(&input, &mut backend, &fast_opts()).expect("runs");
        let report: Value = serde_json::from_str(&raw).expect("valid JSON");
        // EchoBackend returns the full delegated prompt as the result — the
        // task prefix proves substitution while the execution kickoff suffix
        // is intentionally present on first attempts.
        let result = report["phases"][0]["items"][0]["result"]
            .as_str()
            .expect("string result");
        assert!(result.starts_with("task: ship it"));
        assert!(result.contains("[Workflow execution]"));
    }

    #[test]
    fn dollar_input_fanout_uses_array_argument() {
        let input = json!({
            "spec": {
                "name": "demo",
                "phases": [{ "id": "p", "fanout": ["$input"], "prompt": "handle {item}" }]
            },
            "input": ["one", "two", "three"]
        });
        let mut backend = EchoBackend::default();
        let raw = run_workflow_with_backend(&input, &mut backend, &fast_opts()).expect("runs");
        let report: Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(report["agents_spawned"], 3);
    }

    #[test]
    fn workflow_run_writes_replayable_event_log() {
        // Phase-3 end-to-end: the real engine drives the append-only event log,
        // and `read_event_log` reconstructs the run timeline from it alone.
        // Env-free — an explicit path keeps it off the process-global
        // `ZO_WORKFLOW_STORE` (the resume-cache tests' isolation pattern).
        let dir = std::env::temp_dir().join(format!("zo-wf-events-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("e2e.events.jsonl");
        let sink = progress::EventLogSink::with_path("e2e-run".to_string(), Some(path.clone()));
        let opts = RunOptions {
            phase_timeout: Duration::from_millis(50),
            cancel: None,
            cache: None,
            semantic_cache: None,
            worktree: None,
            progress: Some(&sink),
            check: None,
        };
        let input = json!({
            "spec": {
                "name": "demo",
                "phases": [{ "id": "only", "fanout": ["$input"], "prompt": "handle {item}" }]
            },
            "input": ["a", "b"]
        });
        let mut backend = EchoBackend::default();
        run_workflow_with_backend(&input, &mut backend, &opts).expect("runs");

        let events = progress::read_event_log_at(&path);
        assert!(
            events.len() >= 4,
            "expected started + phase enter + agents spawned + phase done + finished, got {}",
            events.len()
        );
        assert!(
            matches!(events.first().map(|e| &e.event),
                Some(WorkflowEventKind::Started { name, .. }) if name == "demo"),
            "first event is the run start"
        );
        let spawned_agents = events
            .iter()
            .filter_map(|event| match &event.event {
                WorkflowEventKind::AgentsSpawned {
                    phase_id,
                    agent_ids,
                } if phase_id == "only" => Some(agent_ids.len()),
                _ => None,
            })
            .sum::<usize>();
        assert_eq!(
            spawned_agents, 2,
            "the fan-out's two agents are recorded against phase `only` across all concurrency batches"
        );
        assert!(
            matches!(events.last().map(|e| &e.event),
                Some(WorkflowEventKind::Finished { status }) if status == "completed"),
            "last event is the terminal completion"
        );
        // run_id + seq give a strict 0..N total order over the appended lines.
        assert_eq!(
            events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            (0..events.len() as u64).collect::<Vec<_>>()
        );
        assert!(events.iter().all(|e| e.run_id == "e2e-run"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_spec_is_rejected() {
        let input = json!({ "name": "", "phases": [] });
        let mut backend = EchoBackend::default();
        let err = run_workflow_with_backend(&input, &mut backend, &fast_opts())
            .expect_err("invalid spec");
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
