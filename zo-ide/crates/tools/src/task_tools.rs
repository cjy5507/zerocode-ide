use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};
use runtime::{task_registry::{Task, TaskRegistry}, PermissionMode, TaskPacket};

#[derive(Debug, Deserialize)]
pub(crate) struct TaskCreateInput {
    pub prompt: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TaskIdInput {
    pub task_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TaskUpdateInput {
    pub task_id: String,
    pub message: String,
}

#[derive(Debug, Deserialize, serde::Serialize, Clone, PartialEq, Eq)]
pub(crate) struct TodoItem {
    /// Stable opaque identifier used to correlate this plan step with a
    /// workflow phase. Optional for backward compatibility with existing
    /// stores and callers.
    #[serde(
        rename = "stepId",
        alias = "step_id",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub step_id: Option<String>,
    pub content: String,
    #[serde(rename = "activeForm")]
    pub active_form: String,
    pub status: TodoStatus,
}

#[derive(Debug, Deserialize, serde::Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TodoWriteInput {
    #[serde(deserialize_with = "deserialize_todos_lenient")]
    pub todos: Vec<TodoItem>,
}

/// Deserialize `todos`, tolerating the stringified-JSON array some models
/// emit (`"[{…}]"`) instead of a real array. Mirrors the leniency already
/// applied to `SpawnMultiAgent`'s `agents`, where strict serde rejected the
/// stringified argument with `invalid type: string, expected a sequence` and
/// forced a wasteful retry round-trip.
fn deserialize_todos_lenient<'de, D>(deserializer: D) -> Result<Vec<TodoItem>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let items = match value {
        Value::Array(items) => items,
        Value::String(raw) => parse_todos_string(&raw).map_err(serde::de::Error::custom)?,
        _ => {
            return Err(serde::de::Error::custom(
                "`todos` must be a JSON array or a JSON-encoded array string",
            ));
        }
    };
    // Re-validate each element against the strict `TodoItem` so the
    // field-level rules (activeForm rename, status enum) still apply.
    items
        .into_iter()
        .map(|item| serde_json::from_value::<TodoItem>(item).map_err(serde::de::Error::custom))
        .collect()
}

/// Parse a stringified `todos` array via [`crate::model_json::parse_model_json`],
/// which repairs the corruptions models reliably produce here: stray invalid
/// JSON escapes (`\d`, Windows paths) and Claude's leaked text-format
/// tool-call closing tags (`</parameter></invoke>`) trailing the array.
fn parse_todos_string(raw: &str) -> Result<Vec<Value>, String> {
    let parsed = crate::model_json::parse_model_json(raw)
        .map_err(|err| format!("`todos` was a string but not valid JSON: {err}"))?;
    match parsed {
        Value::Array(items) => Ok(items),
        _ => Err("`todos` string must encode a JSON array".to_string()),
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct TodoWriteOutput {
    #[serde(rename = "oldTodos")]
    pub old_todos: Vec<TodoItem>,
    #[serde(rename = "newTodos")]
    pub new_todos: Vec<TodoItem>,
    #[serde(rename = "verificationNudgeNeeded")]
    pub verification_nudge_needed: Option<bool>,
    #[serde(rename = "persistenceWarning", skip_serializing_if = "Option::is_none")]
    pub persistence_warning: Option<String>,
}

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    let mut specs = all_task_tool_specs();

    // Task *creation* tools are intentionally not exposed to the model: no
    // runner advances a created task past `Created`, so advertising them would
    // invite the model to delegate work that never executes (hang-over risk).
    // The dispatch arms and `run_task_*` functions remain so existing internal
    // callers (and direct tests) keep working. Read/manage tools stay exposed
    // because they operate safely on whatever records exist.
    specs.retain(|spec| !matches!(spec.name, "TaskCreate" | "RunTaskPacket"));
    specs
}

/// Test-only accessor for the full (unfiltered) task tool spec list, including
/// the hidden creation tools, so spec-string assertions can inspect them.
#[cfg(test)]
pub(crate) fn tool_specs_for_test() -> Vec<ToolSpec> {
    all_task_tool_specs()
}

#[allow(clippy::too_many_lines)] // a flat spec table, clearer unsplit
fn all_task_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "TodoWrite",
            description: "Maintain a concrete checklist for non-trivial work. Keep one item in_progress, revise it as needed, and complete items promptly; skip trivial work. Workflow items reuse stepId as the phase id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "stepId": {
                                    "type": "string",
                                    "description": "Optional matching Workflow phase id."
                                },
                                "content": { "type": "string" },
                                "activeForm": { "type": "string" },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                }
                            },
                            "required": ["content", "activeForm", "status"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["todos"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "TaskCreate",
            description: "Create a tracked task record (Created state). \
                Execution is not yet wired — no runner advances it to Running/Completed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["prompt"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "RunTaskPacket",
            description:
                "Create a tracked task record from a structured task packet (Created state). \
                Execution is not yet wired — no runner advances it to Running/Completed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "objective": { "type": "string" },
                    "scope": { "type": "string" },
                    "repo": { "type": "string" },
                    "branch_policy": { "type": "string" },
                    "acceptance_tests": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "commit_policy": { "type": "string" },
                    "reporting_contract": { "type": "string" },
                    "escalation_policy": { "type": "string" }
                },
                "required": [
                    "objective",
                    "scope",
                    "repo",
                    "branch_policy",
                    "acceptance_tests",
                    "commit_policy",
                    "reporting_contract",
                    "escalation_policy"
                ],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "TaskGet",
            description: "Get the status and details of a background task by ID.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" }
                },
                "required": ["task_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "TaskList",
            description: "List all background tasks and their current status.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "TaskStop",
            description: "Stop a running background task by ID.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" }
                },
                "required": ["task_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "TaskUpdate",
            description: "Send a message or update to a running background task.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["task_id", "message"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "TaskOutput",
            description: "Retrieve the output produced by a background task.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" }
                },
                "required": ["task_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
    ]
}

pub(crate) fn dispatch(
    ctx: &ToolContext,
    enforcer: Option<&runtime::permission_enforcer::PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<String, ToolError>> {
    let reg = &ctx.tasks;
    let session_id = ctx.session_id();
    match name {
        "TodoWrite" => Some(
            maybe_enforce_permission_check(enforcer, name, input)
                .and_then(|()| from_value::<TodoWriteInput>(input).and_then(run_todo_write)),
        ),
        "TaskCreate" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TaskCreateInput>(input).and_then(|i| run_task_create(reg, &i))
            }),
        ),
        "RunTaskPacket" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TaskPacket>(input).and_then(|i| run_task_packet(reg, i))
            }),
        ),
        "TaskGet" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TaskIdInput>(input)
                    .and_then(|i| run_task_get(reg, session_id.as_deref(), &i))
            }),
        ),
        "TaskList" => Some(
            maybe_enforce_permission_check(enforcer, name, input)
                .and_then(|()| run_task_list(reg, session_id.as_deref(), input.clone())),
        ),
        "TaskStop" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TaskIdInput>(input)
                    .and_then(|i| run_task_stop(reg, session_id.as_deref(), &i))
            }),
        ),
        "TaskUpdate" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TaskUpdateInput>(input)
                    .and_then(|i| run_task_update(reg, session_id.as_deref(), &i))
            }),
        ),
        "TaskOutput" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TaskIdInput>(input)
                    .and_then(|i| run_task_output(reg, session_id.as_deref(), &i))
            }),
        ),
        _ => None,
    }
}

fn task_visible_in_session(task: &Task, session_id: Option<&str>) -> bool {
    task.session_id
        .as_deref()
        .is_none_or(|owner| Some(owner) == session_id)
}

fn require_task_session(task: &Task, session_id: Option<&str>) -> Result<(), ToolError> {
    if task_visible_in_session(task, session_id) {
        return Ok(());
    }
    Err(ToolError::Execution(format!(
        "task {} belongs to session '{}'; current session is '{}'",
        task.task_id,
        task.session_id.as_deref().unwrap_or("<none>"),
        session_id.unwrap_or("<none>")
    )))
}

pub(crate) fn run_task_create(
    registry: &TaskRegistry,
    input: &TaskCreateInput,
) -> Result<String, ToolError> {
    let task = registry.create(&input.prompt, input.description.as_deref());
    to_pretty_json(json!({
        "task_id": task.task_id,
        "status": task.status,
        "prompt": task.prompt,
        "description": task.description,
        "task_packet": task.task_packet,
        "created_at": task.created_at
    }))
}

pub(crate) fn run_task_packet(
    registry: &TaskRegistry,
    input: TaskPacket,
) -> Result<String, ToolError> {
    let task = registry
        .create_from_packet(input)
        .map_err(|error| ToolError::Execution(error.to_string()))?;

    to_pretty_json(json!({
        "task_id": task.task_id,
        "status": task.status,
        "prompt": task.prompt,
        "description": task.description,
        "task_packet": task.task_packet,
        "created_at": task.created_at
    }))
}

pub(crate) fn run_task_get(
    registry: &TaskRegistry,
    session_id: Option<&str>,
    input: &TaskIdInput,
) -> Result<String, ToolError> {
    match registry.get(&input.task_id) {
        Some(task) => {
            require_task_session(&task, session_id)?;
            to_pretty_json(json!({
                "task_id": task.task_id,
                "status": task.status,
                "kind": task.kind,
                "prompt": task.prompt,
                "description": task.description,
                "task_packet": task.task_packet,
                "created_at": task.created_at,
                "updated_at": task.updated_at,
                "messages": task.messages,
                "team_id": task.team_id
            }))
        }
        None => to_pretty_json(json!({
            "error": "not_found",
            "task_id": input.task_id,
            "message": format!("No task with ID '{}'. Task IDs start with 'task_'. Use TaskList to see available tasks.", input.task_id),
            "available_tasks": registry.list(None).into_iter()
                .filter(|task| task_visible_in_session(task, session_id))
                .map(|task| task.task_id)
                .collect::<Vec<_>>()
        })),
    }
}

pub(crate) fn run_task_list(
    registry: &TaskRegistry,
    session_id: Option<&str>,
    _input: Value,
) -> Result<String, ToolError> {
    let tasks: Vec<_> = registry
        .list(None)
        .into_iter()
        .filter(|task| task_visible_in_session(task, session_id))
        .map(|t| {
            json!({
                "task_id": t.task_id,
                "status": t.status,
                "kind": t.kind,
                "prompt": t.prompt,
                "description": t.description,
                "task_packet": t.task_packet,
                "created_at": t.created_at,
                "updated_at": t.updated_at,
                "team_id": t.team_id
            })
        })
        .collect();
    to_pretty_json(json!({
        "tasks": tasks,
        "count": tasks.len()
    }))
}

pub(crate) fn run_task_stop(
    registry: &TaskRegistry,
    session_id: Option<&str>,
    input: &TaskIdInput,
) -> Result<String, ToolError> {
    if let Some(task) = registry.get(&input.task_id) {
        require_task_session(&task, session_id)?;
    }
    match registry.stop(&input.task_id) {
        Ok(task) => to_pretty_json(json!({
            "task_id": task.task_id,
            "status": task.status,
            "message": "Task stopped"
        })),
        Err(e) => Err(ToolError::Execution(e)),
    }
}

pub(crate) fn run_task_update(
    registry: &TaskRegistry,
    session_id: Option<&str>,
    input: &TaskUpdateInput,
) -> Result<String, ToolError> {
    if let Some(task) = registry.get(&input.task_id) {
        require_task_session(&task, session_id)?;
    }
    match registry.update(&input.task_id, &input.message) {
        Ok(task) => to_pretty_json(json!({
            "task_id": task.task_id,
            "status": task.status,
            "message_count": task.messages.len(),
            "last_message": input.message
        })),
        Err(e) => Err(ToolError::Execution(e)),
    }
}

pub(crate) fn run_task_output(
    registry: &TaskRegistry,
    session_id: Option<&str>,
    input: &TaskIdInput,
) -> Result<String, ToolError> {
    let task = registry.get(&input.task_id).ok_or_else(|| ToolError::Execution(format!("task not found: {}", input.task_id)))?;
    require_task_session(&task, session_id)?;
    to_pretty_json(json!({
        "task_id": task.task_id,
        "status": task.status,
        "exit_code": task.exit_code,
        "output": task.output,
        "has_output": !task.output.is_empty()
    }))
}

pub(crate) fn run_todo_write(input: TodoWriteInput) -> Result<String, ToolError> {
    to_pretty_json(execute_todo_write(input)?)
}

/// The order of the steps is the model's, in the store and in the result.
/// codex's `update_plan` and Claude Code's todo list both keep it; the engine
/// this was inherited from sorted by status for a HUD, so a finished step sank
/// to the bottom and the in-progress one rose to the top — and each
/// `Updated Plan` cell the TUI drew read as a different plan from the last.
fn execute_todo_write(input: TodoWriteInput) -> Result<TodoWriteOutput, ToolError> {
    validate_todos(&input.todos)?;
    let store_path = todo_store_path()?;
    let old_todos = if store_path.exists() {
        serde_json::from_str::<Vec<TodoItem>>(&std::fs::read_to_string(&store_path)?)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?
    } else {
        Vec::new()
    };

    let new_todos = input.todos;
    let all_done = new_todos
        .iter()
        .all(|todo| matches!(todo.status, TodoStatus::Completed));
    let persisted = if all_done {
        Vec::new()
    } else {
        new_todos.clone()
    };

    // Persist the store, but never let a read-only working directory abort the
    // tool call (and the enclosing `/goal` turn): on EACCES/EROFS for the
    // cwd-relative store, fall back to the per-user zo home; if that is also
    // unwritable, degrade to in-memory — `new_todos` still reaches the model in
    // the result below. Genuine errors (disk full, …) still surface.
    let bytes = serde_json::to_string_pretty(&persisted)?.into_bytes();
    let persistence_warning = persist_todos(
        &store_path,
        fallback_todo_store_path(&store_path).as_deref(),
        &bytes,
    )?;

    let verification_nudge_needed = (all_done
        && new_todos.len() >= 3
        && !new_todos
            .iter()
            .any(|todo| todo.content.to_lowercase().contains("verif")))
    .then_some(true);

    Ok(TodoWriteOutput {
        old_todos,
        new_todos,
        verification_nudge_needed,
        persistence_warning,
    })
}

fn validate_todos(todos: &[TodoItem]) -> Result<(), ToolError> {
    if todos.is_empty() {
        return Err(ToolError::InvalidInput("todos must not be empty".into()));
    }
    // Allow multiple in_progress items for parallel workflows
    if todos.iter().any(|todo| todo.content.trim().is_empty()) {
        return Err(ToolError::InvalidInput(
            "todo content must not be empty".into(),
        ));
    }
    if todos.iter().any(|todo| todo.active_form.trim().is_empty()) {
        return Err(ToolError::InvalidInput(
            "todo activeForm must not be empty".into(),
        ));
    }
    let mut step_ids = std::collections::HashSet::new();
    for step_id in todos.iter().filter_map(|todo| todo.step_id.as_deref()) {
        if step_id.is_empty() || step_id.trim() != step_id {
            return Err(ToolError::InvalidInput(
                "todo stepId must be non-empty and trimmed when provided".into(),
            ));
        }
        if !step_ids.insert(step_id) {
            return Err(ToolError::InvalidInput(format!(
                "todo stepId must be unique: {step_id}"
            )));
        }
    }
    Ok(())
}

fn todo_store_path() -> Result<std::path::PathBuf, ToolError> {
    // Path resolution is owned by `runtime::todo_store` so this writer and the
    // HUD/compaction readers can never drift (the split-brain that hid todos in
    // a read-only cwd).
    let cwd = std::env::current_dir()?;
    Ok(runtime::todo_store::primary_store(&cwd))
}

/// Persist the todo store to `store_path`. A read-only working directory must
/// never fail the tool call — that would abort the enclosing `/goal` turn with
/// a bare OS `Permission denied`. So on `PermissionDenied`/`ReadOnlyFilesystem`
/// we write to `fallback` instead; if that is absent or also unwritable we
/// degrade to in-memory (the todos are still returned to the model). Genuine
/// errors (disk full, a bad path, …) still surface.
fn persist_todos(
    store_path: &std::path::Path,
    fallback: Option<&std::path::Path>,
    bytes: &[u8],
) -> Result<Option<String>, ToolError> {
    match write_todos(store_path, bytes) {
        Ok(()) => Ok(None),
        Err(error) if is_unwritable(&error) => {
            if let Some(fallback) = fallback {
                match write_todos(fallback, bytes) {
                    Ok(()) => return Ok(None),
                    Err(fallback_error) => {
                        eprintln!(
                            "[zo] warning: failed to persist todos to primary store {} ({}) and fallback store {}: {}",
                            store_path.display(),
                            error,
                            fallback.display(),
                            fallback_error
                        );
                        return Ok(Some(format!(
                            "Warning: failed to persist todos to {} after primary store {} was unwritable; todo state may not survive restart: {}",
                            fallback.display(),
                            store_path.display(),
                            fallback_error
                        )));
                    }
                }
            }
            eprintln!(
                "[zo] warning: failed to persist todos at {} and no fallback store was available: {}",
                store_path.display(),
                error
            );
            Ok(Some(format!(
                "Warning: failed to persist todos because {} is unwritable and no fallback store was available; todo state may not survive restart: {}",
                store_path.display(),
                error
            )))
        }
        Err(error) => Err(error.into()),
    }
}

static TODO_TEMP_COUNTER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn write_todos(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    // Crash/power-loss durability has no deterministic in-process reproduction,
    // so a red-first test is impractical for the fsync effect itself.
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    // Plain `fs::write` followed a leaf symlink and updated its target;
    // renaming a sibling temp over the link would instead replace the link
    // itself. Resolve the leaf chain so replacement lands on the real store.
    let destination = resolve_todo_leaf_symlinks(path)?;
    let (temp_path, mut temp_file) = create_todo_temp_file(&destination)?;
    // The fresh temp file is umask-default; carry over an existing store's
    // permissions so replacement does not downgrade e.g. a 0600 store to 0644.
    let write_result = match std::fs::metadata(&destination) {
        Ok(metadata) => temp_file.set_permissions(metadata.permissions()),
        // Only a missing destination (plain create) keeps the umask default;
        // any other stat failure must not silently drop the original's mode.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
    .and_then(|()| temp_file.write_all(bytes))
    .and_then(|()| temp_file.flush())
    .and_then(|()| temp_file.sync_all());
    drop(temp_file);
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    // `std::fs::rename` replaces an existing destination file on Unix
    // (`rename(2)`) and Windows (`MoveFileExW` + `MOVEFILE_REPLACE_EXISTING`).
    match std::fs::rename(&temp_path, &destination) {
        Ok(()) => {
            #[cfg(unix)]
            {
                let parent = destination
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| std::path::Path::new("."));
                // The rename above published the new inode; fsync the directory
                // so the rename survives a crash. Best-effort: some filesystems
                // reject directory fsync, and the file data is already durable.
                let _ = std::fs::File::open(parent).and_then(|dir| dir.sync_all());
            }
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_file(temp_path);
            Err(error)
        }
    }
}

/// Follow a leaf-symlink chain (bounded like Linux `MAXSYMLINKS`) to the store
/// file replacement actually targets. A cycle (or an unreadable/absurdly long
/// chain) refuses with the `ELOOP`-style error plain `fs::write` produced,
/// instead of renaming the temp file over the link itself.
fn resolve_todo_leaf_symlinks(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match current.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&current)?;
                if target.is_absolute() {
                    current = target;
                } else {
                    let base = current.parent().unwrap_or_else(|| std::path::Path::new(""));
                    current = base.join(target);
                }
            }
            Ok(_) => return Ok(current),
            // A missing leaf is the plain-create case; any other lstat failure
            // (e.g. an unsearchable parent) must propagate — proceeding could
            // rename over a path we could not prove is not a symlink.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(current),
            Err(error) => return Err(error),
        }
    }
    // The hop budget is spent. A chain of exactly 40 links that settled on a
    // non-symlink is within the budget (Linux errors on the 41st hop, not the
    // 40th); refuse only when the path is STILL a symlink — a cycle or an
    // over-budget chain. `ErrorKind::FilesystemLoop` is still unstable
    // (`io_error_more`), so the ELOOP meaning travels in the message.
    match current.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => Err(std::io::Error::other(format!(
            "too many levels of symbolic links resolving {}",
            path.display()
        ))),
        Ok(_) => Ok(current),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(current),
        Err(error) => Err(error),
    }
}

fn create_todo_temp_file(
    path: &std::path::Path,
) -> std::io::Result<(std::path::PathBuf, std::fs::File)> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("todos.json");
    for _ in 0..128 {
        let counter = TODO_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{name}.tmp-{}-{counter}",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("could not allocate a temporary todo file for {}", path.display()),
    ))
}

/// Errors meaning "this location is not writable by us" — a different target
/// would help, unlike a genuine failure we must surface.
fn is_unwritable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
    )
}

/// `<zo home>/orphan-todos/<key>.json` — the writable per-user fallback used
/// when the primary (cwd) store is read-only. Reuses the same zo-home chain
/// (`ZO_CONFIG_HOME`/`ZO_HOME`/`HOME`) that sessions already persist under.
fn fallback_todo_store_path(store_path: &std::path::Path) -> Option<std::path::PathBuf> {
    runtime::todo_store::fallback_store(store_path)
}

/// A stable, collision-resistant filename stem for a store path, so distinct
/// projects never clobber each other's fallback store. A naive
/// non-alphanumeric→`_` mapping is **not injective** (`/a/b` and `/a-b` would
/// collide) and can be empty, so hash the full path string instead. The hasher
/// is fixed-seeded, so the same path always maps to the same file across runs.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_tools_reject_cross_session_access_and_list_legacy_tasks() {
        let registry = TaskRegistry::new_in_memory();
        let foreign = registry.create_background_process(
            "sleep 30",
            Some("background bash"),
            Some("session-a"),
        );
        let local = registry.create_background_process(
            "sleep 20",
            Some("local background bash"),
            Some("session-b"),
        );
        let legacy = registry.create("legacy task", None);
        let ctx = ToolContext::new().with_tasks(registry.clone());
        ctx.set_session_id("session-b");

        let listed = dispatch(&ctx, None, "TaskList", &json!({}))
            .expect("TaskList should dispatch")
            .expect("TaskList should succeed");
        let listed: Value = serde_json::from_str(&listed).expect("TaskList json");
        let task_ids: Vec<_> = listed["tasks"]
            .as_array()
            .expect("tasks array")
            .iter()
            .filter_map(|task| task["task_id"].as_str())
            .collect();
        assert!(task_ids.contains(&legacy.task_id.as_str()));
        assert!(task_ids.contains(&local.task_id.as_str()));
        assert!(!task_ids.contains(&foreign.task_id.as_str()));
        let listed_local = listed["tasks"]
            .as_array()
            .expect("tasks array")
            .iter()
            .find(|task| task["task_id"] == local.task_id)
            .expect("local background task should be listed");
        assert_eq!(listed_local["kind"], "background_process");

        let fetched = dispatch(
            &ctx,
            None,
            "TaskGet",
            &json!({"task_id": local.task_id}),
        )
        .expect("TaskGet should dispatch")
        .expect("TaskGet should succeed");
        let fetched: Value = serde_json::from_str(&fetched).expect("TaskGet json");
        assert_eq!(fetched["kind"], "background_process");

        for (tool, input) in [
            ("TaskGet", json!({"task_id": foreign.task_id})),
            ("TaskOutput", json!({"task_id": foreign.task_id})),
            ("TaskStop", json!({"task_id": foreign.task_id})),
            (
                "TaskUpdate",
                json!({"task_id": foreign.task_id, "message": "cross-session"}),
            ),
        ] {
            let error = dispatch(&ctx, None, tool, &input)
                .expect("task tool should dispatch")
                .expect_err("cross-session access must fail");
            let message = error.to_string();
            assert!(message.contains("belongs to session 'session-a'"), "{message}");
            assert!(message.contains("current session is 'session-b'"), "{message}");
        }

        assert_eq!(
            registry.get(&foreign.task_id).map(|task| task.status),
            Some(runtime::task_registry::TaskStatus::Created),
            "a rejected TaskStop must not mutate the foreign task"
        );
    }

    #[test]
    fn todo_write_accepts_real_array() {
        let input = json!({
            "todos": [
                {"stepId": "build", "content": "build it", "activeForm": "building it", "status": "in_progress"}
            ]
        });
        let parsed: TodoWriteInput = serde_json::from_value(input).expect("real array must parse");
        assert_eq!(parsed.todos.len(), 1);
        assert_eq!(parsed.todos[0].step_id.as_deref(), Some("build"));
        assert_eq!(parsed.todos[0].active_form, "building it");
        assert_eq!(parsed.todos[0].status, TodoStatus::InProgress);
    }

    #[test]
    fn todo_write_accepts_stringified_array() {
        // Some models serialize the whole `todos` argument to a string
        // ("[{…}]") instead of a real array; strict serde rejected that with
        // `invalid type: string, expected a sequence`. We now coerce it.
        let input = json!({
            "todos": "[{\"stepId\":\"do\",\"content\":\"do it\",\"activeForm\":\"doing it\",\"status\":\"pending\"}]"
        });
        let parsed: TodoWriteInput =
            serde_json::from_value(input).expect("stringified array must parse");
        assert_eq!(parsed.todos.len(), 1);
        assert_eq!(parsed.todos[0].step_id.as_deref(), Some("do"));
        assert_eq!(parsed.todos[0].content, "do it");
        assert_eq!(parsed.todos[0].status, TodoStatus::Pending);
    }

    #[test]
    fn todo_write_rejects_non_json_string() {
        let input = json!({ "todos": "not json at all" });
        let err =
            serde_json::from_value::<TodoWriteInput>(input).expect_err("garbage string must fail");
        assert!(
            err.to_string().contains("not valid JSON"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn todo_write_rejects_malformed_item_in_stringified_array() {
        // A stringified array whose items are missing required fields must
        // still fail — each element is re-validated against `TodoItem`.
        let input = json!({ "todos": "[{\"content\":\"x\"}]" });
        let err =
            serde_json::from_value::<TodoWriteInput>(input).expect_err("missing fields must fail");
        assert!(
            err.to_string().contains("activeForm") || err.to_string().contains("missing field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn todo_write_repairs_stray_backslash_escape_in_stringified_array() {
        // Models that hand-stringify the array and include a regex / Windows
        // path produce an invalid JSON escape (`\d`, `\(`, `\C`) that strict
        // serde rejects with `invalid escape at …`. The lenient path repairs
        // the stray backslashes so the (otherwise valid) tool call succeeds.
        let input = json!({
            "todos": "[{\"content\":\"fix regex \\d+ and path C:\\Users\",\"activeForm\":\"fixing\",\"status\":\"pending\"}]"
        });
        let parsed: TodoWriteInput =
            serde_json::from_value(input).expect("stray-escape array must parse after repair");
        assert_eq!(parsed.todos.len(), 1);
        assert_eq!(parsed.todos[0].step_id, None, "legacy items stay valid");
        assert_eq!(parsed.todos[0].content, r"fix regex \d+ and path C:\Users");
    }

    #[test]
    fn todo_write_rejects_blank_or_duplicate_step_ids() {
        let todo = |step_id: &str| TodoItem {
            step_id: Some(step_id.to_string()),
            content: format!("step {step_id}"),
            active_form: format!("doing {step_id}"),
            status: TodoStatus::Pending,
        };
        assert!(validate_todos(&[todo(" ")]).is_err());
        let duplicate = validate_todos(&[todo("same"), todo("same")])
            .expect_err("duplicate step ids are ambiguous");
        assert!(duplicate.to_string().contains("must be unique"));
    }

    #[test]
    fn is_unwritable_classifies_permission_and_readonly() {
        use std::io::{Error, ErrorKind};
        assert!(is_unwritable(&Error::from(ErrorKind::PermissionDenied)));
        assert!(is_unwritable(&Error::from(ErrorKind::ReadOnlyFilesystem)));
        assert!(!is_unwritable(&Error::from(ErrorKind::NotFound)));
        assert!(!is_unwritable(&Error::from(ErrorKind::AlreadyExists)));
    }

    #[test]
    fn orphan_todo_key_is_injective_stable_and_nonempty() {
        use std::path::Path;
        // Paths the old non-alphanumeric→`_` sanitizer collided must now differ.
        use runtime::todo_store::orphan_todo_key;
        // Paths the old non-alphanumeric sanitizer collided must now differ.
        assert_ne!(
            orphan_todo_key(Path::new("/a/b")),
            orphan_todo_key(Path::new("/a-b"))
        );
        assert_ne!(
            orphan_todo_key(Path::new("/home/u/proj-a/.zo-todos.json")),
            orphan_todo_key(Path::new("/home/u/proj_a/.zo-todos.json"))
        );
        assert_eq!(
            orphan_todo_key(Path::new("/x/y")),
            orphan_todo_key(Path::new("/x/y"))
        );
        assert!(!orphan_todo_key(Path::new("")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn todo_write_preserves_store_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("create temp directory");
        let store_path = temp.path().join("todos.json");
        std::fs::write(&store_path, b"[]").expect("seed todo store");
        std::fs::set_permissions(&store_path, std::fs::Permissions::from_mode(0o600))
            .expect("restrict todo store mode");

        super::write_todos(&store_path, b"[\"updated\"]").expect("replace todo store");

        let mode = std::fs::metadata(&store_path)
            .expect("stat todo store")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "todo replacement must not downgrade the store mode"
        );
    }

    #[cfg(unix)]
    #[test]
    fn todo_write_follows_leaf_symlink() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let target = temp.path().join("real-todos.json");
        std::fs::write(&target, b"[]").expect("seed symlink target");
        let link = temp.path().join("todos-link.json");
        std::os::unix::fs::symlink(&target, &link).expect("create leaf symlink");

        super::write_todos(&link, b"[\"updated\"]").expect("replace through symlink");

        assert!(
            link.symlink_metadata()
                .expect("lstat todo link")
                .file_type()
                .is_symlink(),
            "the store symlink must survive replacement"
        );
        assert_eq!(
            std::fs::read(&target).expect("read symlink target"),
            b"[\"updated\"]",
            "the write must land on the symlink's target"
        );
    }

    #[cfg(unix)]
    #[test]
    fn todo_write_follows_exactly_forty_symlink_hops() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let real = temp.path().join("real-todos.json");
        std::fs::write(&real, b"[]").expect("seed chain target");
        let mut previous = real.clone();
        for i in 1..=40 {
            let link = temp.path().join(format!("link-{i}.json"));
            std::os::unix::fs::symlink(&previous, &link).expect("create chain link");
            previous = link;
        }
        let head = previous;

        super::write_todos(&head, b"[\"updated\"]")
            .expect("a 40-hop chain is within the budget");

        assert_eq!(
            std::fs::read(&real).expect("read chain target"),
            b"[\"updated\"]",
            "the write must land on the chain's final target"
        );
        assert!(
            head.symlink_metadata()
                .expect("lstat chain head")
                .file_type()
                .is_symlink(),
            "the chain head must remain a symlink"
        );
    }

    #[cfg(unix)]
    #[test]
    fn todo_resolver_propagates_unreadable_parent_errors() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("create temp directory");
        let locked = temp.path().join("locked");
        std::fs::create_dir(&locked).expect("create locked directory");
        let target = locked.join("todos.json");
        std::fs::write(&target, b"[]").expect("seed target");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
            .expect("lock parent directory");

        // Root ignores permission bits: probe and skip instead of failing.
        if target.symlink_metadata().is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
                .expect("restore parent permissions");
            return;
        }

        let result = super::resolve_todo_leaf_symlinks(&target);

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
            .expect("restore parent permissions");
        assert!(
            result.is_err(),
            "an unreadable parent must propagate its error, not fail open as a regular file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn todo_write_refuses_symlink_cycle() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let a = temp.path().join("a-link.json");
        let b = temp.path().join("b-link.json");
        std::os::unix::fs::symlink(&b, &a).expect("create a->b");
        std::os::unix::fs::symlink(&a, &b).expect("create b->a");

        let result = super::write_todos(&a, b"[\"updated\"]");

        assert!(result.is_err(), "a symlink cycle must refuse replacement");
        assert!(
            a.symlink_metadata()
                .expect("lstat cyclic link")
                .file_type()
                .is_symlink(),
            "the cyclic link must be left intact, not renamed over"
        );
    }

    #[cfg(unix)]
    #[test]
    fn todo_atomic_write_failure_preserves_existing_plan() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("create temp directory");
        let store_dir = temp.path().join("store");
        std::fs::create_dir(&store_dir).expect("create store directory");
        let store_path = store_dir.join("todos.json");
        let original = vec![TodoItem {
            step_id: Some("original".to_string()),
            content: "original plan".to_string(),
            active_form: "keeping original plan".to_string(),
            status: TodoStatus::Pending,
        }];
        std::fs::write(
            &store_path,
            serde_json::to_vec_pretty(&original).expect("serialize original plan"),
        )
        .expect("seed original plan");
        std::fs::set_permissions(&store_dir, std::fs::Permissions::from_mode(0o555))
            .expect("make store directory read-only");

        let probe = store_dir.join("probe");
        if std::fs::write(&probe, b"probe").is_ok() {
            let _ = std::fs::remove_file(probe);
            std::fs::set_permissions(&store_dir, std::fs::Permissions::from_mode(0o755))
                .expect("restore store permissions");
            return;
        }

        let replacement = serde_json::to_vec_pretty(&vec![TodoItem {
            step_id: Some("replacement".to_string()),
            content: "replacement plan".to_string(),
            active_form: "writing replacement plan".to_string(),
            status: TodoStatus::InProgress,
        }])
        .expect("serialize replacement plan");
        let result = write_todos(&store_path, &replacement);

        std::fs::set_permissions(&store_dir, std::fs::Permissions::from_mode(0o755))
            .expect("restore store permissions");
        let persisted: Vec<TodoItem> = serde_json::from_slice(
            &std::fs::read(&store_path).expect("read original plan after failed write"),
        )
        .expect("original plan remains valid JSON");
        assert!(result.is_err(), "creating the sibling temp file must fail");
        assert_eq!(persisted, original, "failed persistence must preserve the plan");
    }

    #[cfg(unix)]
    #[test]
    fn persist_todos_falls_back_when_cwd_is_read_only() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("zo-todo-ro-{}", std::process::id()));
        let ro_dir = base.join("ro");
        let fb_dir = base.join("fb");
        std::fs::create_dir_all(&ro_dir).expect("mk ro");
        std::fs::create_dir_all(&fb_dir).expect("mk fb");
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o555))
            .expect("chmod ro");

        // Skip when this uid can write to a 0555 dir anyway (e.g. running as
        // root): the EACCES we are reproducing cannot occur.
        if std::fs::write(ro_dir.join(".probe"), b"x").is_ok() {
            let _ = std::fs::remove_file(ro_dir.join(".probe"));
            let _ = std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::remove_dir_all(&base);
            return;
        }

        let primary = ro_dir.join(".zo-todos.json");
        let fallback = fb_dir.join("orphan.json");

        // Reproduces the original bug: the raw write propagates EACCES out of
        // the tool call \u2026
        assert!(
            write_todos(&primary, b"[]").is_err(),
            "raw write to a read-only dir should fail"
        );
        // \u2026 and the fix swallows it, persisting to the fallback instead of
        // aborting the /goal turn.
        let result = persist_todos(&primary, Some(fallback.as_path()), b"[]");
        assert!(
            matches!(result, Ok(None)),
            "read-only cwd must not fail or warn when fallback succeeds: {result:?}"
        );
        assert!(!primary.exists(), "primary must not have been written");
        assert_eq!(
            std::fs::read(&fallback).expect("fallback written"),
            b"[]",
            "todos must land in the fallback store"
        );

        let _ = std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&base);
    }
}
