use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};
use runtime::{
    worker_boot::{WorkerReadySnapshot, WorkerRegistry},
    PermissionMode,
};

#[derive(Debug, Deserialize)]
pub(crate) struct WorkerCreateInput {
    pub cwd: String,
    #[serde(default)]
    pub trusted_roots: Vec<String>,
    #[serde(default = "default_auto_recover_prompt_misdelivery")]
    pub auto_recover_prompt_misdelivery: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkerIdInput {
    pub worker_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkerObserveInput {
    pub worker_id: String,
    pub screen_text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkerSendPromptInput {
    pub worker_id: String,
    #[serde(default)]
    pub prompt: Option<String>,
}

const fn default_auto_recover_prompt_misdelivery() -> bool {
    true
}

#[allow(clippy::too_many_lines)] // a flat spec table, clearer unsplit
pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "WorkerCreate",
            description: "Create a coding worker boot session with trust-gate and prompt-delivery guards.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "cwd": { "type": "string" },
                    "trusted_roots": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "auto_recover_prompt_misdelivery": { "type": "boolean" }
                },
                "required": ["cwd"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "WorkerGet",
            description: "Fetch the current worker boot state, last error, and event history.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "worker_id": { "type": "string" }
                },
                "required": ["worker_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "WorkerObserve",
            description: "Feed a terminal snapshot into worker boot detection to resolve trust gates, ready handshakes, and prompt misdelivery.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "worker_id": { "type": "string" },
                    "screen_text": { "type": "string" }
                },
                "required": ["worker_id", "screen_text"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "WorkerResolveTrust",
            description: "Resolve a detected trust prompt so worker boot can continue.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "worker_id": { "type": "string" }
                },
                "required": ["worker_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "WorkerAwaitReady",
            description: "Return the current ready-handshake verdict for a coding worker.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "worker_id": { "type": "string" }
                },
                "required": ["worker_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "WorkerSendPrompt",
            description: "Send a task prompt only after the worker reaches ready_for_prompt; can replay a recovered prompt.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "worker_id": { "type": "string" },
                    "prompt": { "type": "string" }
                },
                "required": ["worker_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "WorkerRestart",
            description: "Restart worker boot state after a failed or stale startup.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "worker_id": { "type": "string" }
                },
                "required": ["worker_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "WorkerTerminate",
            description: "Terminate a worker and mark the lane finished from the control plane.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "worker_id": { "type": "string" }
                },
                "required": ["worker_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
    ]
}

pub(crate) fn dispatch(
    ctx: &ToolContext,
    enforcer: Option<&runtime::permission_enforcer::PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<String, ToolError>> {
    let reg = &ctx.workers;
    match name {
        "WorkerCreate" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WorkerCreateInput>(input).and_then(|i| run_worker_create(reg, &i))
            }),
        ),
        "WorkerGet" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WorkerIdInput>(input).and_then(|i| run_worker_get(reg, &i))
            }),
        ),
        "WorkerObserve" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WorkerObserveInput>(input).and_then(|i| run_worker_observe(reg, &i))
            }),
        ),
        "WorkerResolveTrust" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WorkerIdInput>(input).and_then(|i| run_worker_resolve_trust(reg, &i))
            }),
        ),
        "WorkerAwaitReady" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WorkerIdInput>(input).and_then(|i| run_worker_await_ready(reg, &i))
            }),
        ),
        "WorkerSendPrompt" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WorkerSendPromptInput>(input)
                    .and_then(|i| run_worker_send_prompt(reg, &i))
            }),
        ),
        "WorkerRestart" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WorkerIdInput>(input).and_then(|i| run_worker_restart(reg, &i))
            }),
        ),
        "WorkerTerminate" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WorkerIdInput>(input).and_then(|i| run_worker_terminate(reg, &i))
            }),
        ),
        _ => None,
    }
}

fn run_worker_create(reg: &WorkerRegistry, input: &WorkerCreateInput) -> Result<String, ToolError> {
    to_pretty_json(reg.create(
        &input.cwd,
        &input.trusted_roots,
        input.auto_recover_prompt_misdelivery,
    ))
}

fn run_worker_get(reg: &WorkerRegistry, input: &WorkerIdInput) -> Result<String, ToolError> {
    reg.get(&input.worker_id).map_or_else(
        || {
            Err(ToolError::NotFound(format!(
                "worker not found: {}",
                input.worker_id
            )))
        },
        to_pretty_json,
    )
}

fn run_worker_observe(
    reg: &WorkerRegistry,
    input: &WorkerObserveInput,
) -> Result<String, ToolError> {
    to_pretty_json(
        reg.observe(&input.worker_id, &input.screen_text)
            .map_err(ToolError::Execution)?,
    )
}

fn run_worker_resolve_trust(
    reg: &WorkerRegistry,
    input: &WorkerIdInput,
) -> Result<String, ToolError> {
    to_pretty_json(
        reg.resolve_trust(&input.worker_id)
            .map_err(ToolError::Execution)?,
    )
}

fn run_worker_await_ready(
    reg: &WorkerRegistry,
    input: &WorkerIdInput,
) -> Result<String, ToolError> {
    let snapshot: WorkerReadySnapshot = reg
        .await_ready(&input.worker_id)
        .map_err(ToolError::Execution)?;
    to_pretty_json(snapshot)
}

fn run_worker_send_prompt(
    reg: &WorkerRegistry,
    input: &WorkerSendPromptInput,
) -> Result<String, ToolError> {
    to_pretty_json(
        reg.send_prompt(&input.worker_id, input.prompt.as_deref())
            .map_err(ToolError::Execution)?,
    )
}

fn run_worker_restart(reg: &WorkerRegistry, input: &WorkerIdInput) -> Result<String, ToolError> {
    to_pretty_json(
        reg.restart(&input.worker_id)
            .map_err(ToolError::Execution)?,
    )
}

fn run_worker_terminate(reg: &WorkerRegistry, input: &WorkerIdInput) -> Result<String, ToolError> {
    to_pretty_json(
        reg.terminate(&input.worker_id)
            .map_err(ToolError::Execution)?,
    )
}
