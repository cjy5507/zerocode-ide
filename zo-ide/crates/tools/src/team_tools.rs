use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::team_inbox_store::{
    NewTeamUpdate, StoreMode, TeamInboxChannel, TeamInboxDeliveryState, TeamInboxPriority,
    TeamInboxStore, TeamInboxStoreError, TeamUpdate,
};

use super::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};
use runtime::{
    task_registry::{TaskRegistry, TaskStatus},
    team_cron_registry::{
        cron_zone, validate_cron_schedule, CronRegistry, TeamRegistry, AUTOMATIC_SCHEDULER_NOTE,
        AUTOMATIC_SCHEDULER_STATUS,
    },
    PermissionMode,
};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamCreateInput {
    pub name: String,
    pub tasks: Vec<TeamTaskInput>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum TeamTaskInput {
    ExistingTask {
        task_id: String,
    },
    InlineTask {
        prompt: String,
        #[serde(default)]
        description: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamDeleteInput {
    pub team_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CronCreateInput {
    pub schedule: String,
    pub prompt: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CronDeleteInput {
    pub cron_id: String,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct CronRunDueInput {
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamInboxPostInput {
    #[serde(default)]
    pub id: Option<String>,
    pub channel: String,
    pub source: String,
    pub summary: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub priority: Option<TeamInboxPriority>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamInboxJoinInput {
    pub consumer_id: String,
    pub channel: String,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct TeamInboxChannelsInput {
    #[serde(default)]
    pub consumer_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamInboxUnreadInput {
    pub consumer_id: String,
    pub channel: String,
    #[serde(default = "default_team_inbox_unread_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamInboxAckInput {
    pub consumer_id: String,
    pub channel: String,
    pub update_id: String,
    pub turn_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TeamInboxLeaveInput {
    pub consumer_id: String,
    pub channel: String,
}

fn default_team_inbox_unread_limit() -> usize {
    10
}

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    let mut specs = team_inbox_tool_specs();
    specs.extend([
        ToolSpec {
            name: "TeamCreate",
            description: "Create a team record and associate tracked task records. Execution is not yet wired; use Agent, SpawnMultiAgent, or Workflow for actual agent execution.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "tasks": {
                        "type": "array",
                        "items": {
                            "anyOf": [
                                {
                                    "type": "object",
                                    "properties": {
                                        "task_id": { "type": "string" }
                                    },
                                    "required": ["task_id"],
                                    "additionalProperties": false
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "prompt": { "type": "string" },
                                        "description": { "type": "string" }
                                    },
                                    "required": ["prompt"],
                                    "additionalProperties": false
                                }
                            ]
                        }
                    }
                },
                "required": ["name", "tasks"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "TeamDelete",
            description: "Delete a team record and stop all non-terminal tracked tasks.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "team_id": { "type": "string" }
                },
                "required": ["team_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
    ]);
    specs.extend(cron_tool_specs());
    specs
}

fn team_inbox_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "TeamInboxPost",
            description: "Post a low-trust TeamInbox update to a local channel. Large bodies are stored as artifact refs; no prompt injection happens in this tool. Runtime separately injects a low-trust summary digest at turn start for subscribed session consumers and auto-acks after successful turns.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "channel": { "type": "string" },
                    "source": { "type": "string" },
                    "summary": { "type": "string" },
                    "body": { "type": "string" },
                    "priority": { "type": "string", "enum": ["low", "normal", "high"] },
                    "task_id": { "type": "string" },
                    "status": { "type": "string" }
                },
                "required": ["channel", "source", "summary"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "TeamInboxJoin",
            description: "Subscribe a consumer to a TeamInbox channel from the current tail; existing backlog is skipped.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "consumer_id": { "type": "string" },
                    "channel": { "type": "string" }
                },
                "required": ["consumer_id", "channel"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "TeamInboxChannels",
            description: "List TeamInbox channels and optional cursor state for a non-runtime consumer.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "consumer_id": { "type": "string" }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "TeamInboxUnread",
            description: "Read TeamInbox updates after a consumer cursor. Returns summaries and artifact refs only; raw body text is not emitted.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "consumer_id": { "type": "string" },
                    "channel": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                },
                "required": ["consumer_id", "channel"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "TeamInboxAck",
            description: "Acknowledge a TeamInbox update after it was manually incorporated in a turn. This thin wrapper records Injected then Acked for the supplied turn_id; it does not inject prompt context. Runtime separately injects a low-trust summary digest at turn start for subscribed session consumers and auto-acks after successful turns.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "consumer_id": { "type": "string" },
                    "channel": { "type": "string" },
                    "update_id": { "type": "string" },
                    "turn_id": { "type": "string" }
                },
                "required": ["consumer_id", "channel", "update_id", "turn_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
        ToolSpec {
            name: "TeamInboxLeave",
            description: "Unsubscribe a non-runtime consumer from a TeamInbox channel; delivery history is kept.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "consumer_id": { "type": "string" },
                    "channel": { "type": "string" }
                },
                "required": ["consumer_id", "channel"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
    ]
}

fn cron_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "CronCreate",
            description: "Register a cron-like recurring task record. When this session is idle and the schedule is due, a turn opens with the prompt; CronRunDue also enqueues currently due enabled crons by hand as Created task records.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "schedule": { "type": "string", "description": "Five-field cron read in this session's own time zone: minute hour day-of-month month day-of-week. The wall clock you write is the wall clock it fires on; the receipt names the zone." },
                    "prompt": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["schedule", "prompt"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "CronDelete",
            description: "Permanently remove a registered cron task record by ID.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "cron_id": { "type": "string" }
                },
                "required": ["cron_id"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "CronList",
            description: "List all registered cron task records, their next due time, the time zone their schedules are read in, and the scheduler status.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "CronRunDue",
            description: "Manually enqueue currently due enabled cron records as Created task records — the by-hand path beside the idle session's own scheduler. Does not execute agents or run tasks itself; missed runs are coalesced to one task per cron per invocation.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dry_run": { "type": "boolean" }
                },
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
    match name {
        "TeamInboxPost" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TeamInboxPostInput>(input)
                    .and_then(|parsed| run_team_inbox_post(ctx, parsed))
            }),
        ),
        "TeamInboxJoin" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TeamInboxJoinInput>(input)
                    .and_then(|parsed| run_team_inbox_join(ctx, &parsed))
            }),
        ),
        "TeamInboxChannels" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TeamInboxChannelsInput>(input)
                    .and_then(|parsed| run_team_inbox_channels(ctx, &parsed))
            }),
        ),
        "TeamInboxUnread" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TeamInboxUnreadInput>(input)
                    .and_then(|parsed| run_team_inbox_unread(ctx, &parsed))
            }),
        ),
        "TeamInboxAck" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TeamInboxAckInput>(input)
                    .and_then(|parsed| run_team_inbox_ack(ctx, &parsed))
            }),
        ),
        "TeamInboxLeave" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TeamInboxLeaveInput>(input)
                    .and_then(|parsed| run_team_inbox_leave(ctx, &parsed))
            }),
        ),
        "TeamCreate" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TeamCreateInput>(input)
                    .and_then(|parsed| run_team_create(&ctx.teams, &ctx.tasks, parsed))
            }),
        ),
        "TeamDelete" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<TeamDeleteInput>(input)
                    .and_then(|parsed| run_team_delete(&ctx.teams, &ctx.tasks, &parsed))
            }),
        ),
        "CronCreate" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<CronCreateInput>(input)
                    .and_then(|parsed| run_cron_create(&ctx.crons, &parsed))
            }),
        ),
        "CronDelete" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<CronDeleteInput>(input)
                    .and_then(|parsed| run_cron_delete(&ctx.crons, &parsed))
            }),
        ),
        "CronList" => Some(
            maybe_enforce_permission_check(enforcer, name, input)
                .and_then(|()| run_cron_list(&ctx.crons)),
        ),
        "CronRunDue" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<CronRunDueInput>(input)
                    .and_then(|parsed| run_cron_run_due(&ctx.crons, &ctx.tasks, &parsed))
            }),
        ),
        _ => None,
    }
}

#[derive(Debug)]
struct InlineTaskSpec {
    prompt: String,
    description: Option<String>,
}

fn resolve_team_task_ids(
    tasks: &TaskRegistry,
    team_tasks: Vec<TeamTaskInput>,
) -> Result<Vec<String>, ToolError> {
    if team_tasks.is_empty() {
        return Err(ToolError::InvalidInput(
            "tasks must contain at least one task".into(),
        ));
    }

    let mut task_ids = Vec::with_capacity(team_tasks.len());
    let mut seen_existing_task_ids = BTreeSet::new();
    let mut inline_specs = Vec::new();

    for team_task in team_tasks {
        match team_task {
            TeamTaskInput::ExistingTask { task_id } => {
                let task = tasks
                    .get(&task_id)
                    .ok_or_else(|| ToolError::NotFound(format!("task not found: {task_id}")))?;
                if let Some(team_id) = task.team_id {
                    return Err(ToolError::InvalidInput(format!(
                        "task {task_id} already belongs to team {team_id}"
                    )));
                }
                if !seen_existing_task_ids.insert(task_id.clone()) {
                    return Err(ToolError::InvalidInput(format!(
                        "duplicate task reference in team input: {task_id}"
                    )));
                }
                task_ids.push(task_id);
            }
            TeamTaskInput::InlineTask {
                prompt,
                description,
            } => inline_specs.push(InlineTaskSpec {
                prompt,
                description,
            }),
        }
    }

    for inline_spec in inline_specs {
        let task = tasks.create(&inline_spec.prompt, inline_spec.description.as_deref());
        task_ids.push(task.task_id);
    }

    Ok(task_ids)
}

fn run_team_inbox_post(ctx: &ToolContext, input: TeamInboxPostInput) -> Result<String, ToolError> {
    run_team_inbox_post_at(team_inbox_store_root(ctx), input)
}

fn run_team_inbox_join(ctx: &ToolContext, input: &TeamInboxJoinInput) -> Result<String, ToolError> {
    run_team_inbox_join_at(team_inbox_store_root(ctx), input)
}

fn run_team_inbox_channels(
    ctx: &ToolContext,
    input: &TeamInboxChannelsInput,
) -> Result<String, ToolError> {
    run_team_inbox_channels_at(team_inbox_store_root(ctx), input)
}

fn run_team_inbox_unread(ctx: &ToolContext, input: &TeamInboxUnreadInput) -> Result<String, ToolError> {
    run_team_inbox_unread_at(team_inbox_store_root(ctx), input)
}

fn run_team_inbox_ack(ctx: &ToolContext, input: &TeamInboxAckInput) -> Result<String, ToolError> {
    run_team_inbox_ack_at(team_inbox_store_root(ctx), input)
}

fn run_team_inbox_leave(ctx: &ToolContext, input: &TeamInboxLeaveInput) -> Result<String, ToolError> {
    run_team_inbox_leave_at(team_inbox_store_root(ctx), input)
}

fn run_team_inbox_post_at(
    root: impl AsRef<Path>,
    input: TeamInboxPostInput,
) -> Result<String, ToolError> {
    let mut store = TeamInboxStore::open_at(root);
    let id = input
        .id
        .as_ref()
        .filter(|id| !id.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| generated_team_update_id(&input));
    let update = store
        .post_update(NewTeamUpdate {
            id,
            channel: input.channel,
            source: input.source,
            created_at_unix: now_secs(),
            priority: input.priority.unwrap_or(TeamInboxPriority::Normal),
            summary: input.summary,
            body: input.body,
            task_id: input.task_id,
            status: input.status,
        })
        .map_err(map_team_inbox_error)?;

    to_pretty_json(json!({
        "status": "posted",
        "store_mode": store_mode_label(store.mode()),
        "update": update_to_tool_json(&update),
        "message": "TeamInbox update posted; no prompt injection was performed"
    }))
}

/// Ensure the `TeamInbox` `SQLite` store exists at `root`, creating and migrating
/// it if missing. Host-side seam for the autonomous-loop digest path: a
/// `runtime`-side session subscription must be able to join a channel *before*
/// the first update is posted, which needs the store (and its schema) to already
/// exist. A no-op when the store is already present. Errors are returned for the
/// host to log; callers treat inbox trouble as fail-open.
pub fn ensure_team_inbox_store(root: &Path) -> Result<(), String> {
    // `open_at` creates the directory + `SQLite` schema (runs migrations) as a
    // side effect; a `ReadWrite` mode confirms the store is usable.
    match TeamInboxStore::open_at(root).mode() {
        StoreMode::ReadWrite => Ok(()),
        StoreMode::ReadOnly => {
            Err("TeamInbox store is unavailable (read-only jsonl fallback)".to_string())
        }
    }
}

/// Post a `TeamInbox` update from a non-tool host caller (the CLI autonomous
/// loop's budget-exhausted pause notice). Creates the store if missing. Mirrors
/// the `TeamInboxPost` tool's write path but takes an explicit root and plain
/// fields — no `ToolContext`, no permission check, no prompt injection. Errors
/// are returned for the host to log; callers treat inbox trouble as fail-open so
/// a post failure never blocks a turn.
pub fn host_post_team_inbox_update(
    root: &Path,
    channel: &str,
    source: &str,
    summary: &str,
) -> Result<(), String> {
    run_team_inbox_post_at(
        root,
        TeamInboxPostInput {
            id: None,
            channel: channel.to_string(),
            source: source.to_string(),
            summary: summary.to_string(),
            body: None,
            priority: None,
            task_id: None,
            status: None,
        },
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn run_team_inbox_join_at(
    root: impl AsRef<Path>,
    input: &TeamInboxJoinInput,
) -> Result<String, ToolError> {
    reject_reserved_team_inbox_consumer(&input.consumer_id)?;
    let mut store = TeamInboxStore::open_at(root);
    let last_seen_seq = store
        .join_channel_from_now(&input.consumer_id, &input.channel)
        .map_err(map_team_inbox_error)?;
    to_pretty_json(json!({
        "status": "joined_from_now",
        "store_mode": store_mode_label(store.mode()),
        "consumer_id": input.consumer_id,
        "channel": input.channel,
        "last_seen_seq": last_seen_seq,
        "message": "Existing backlog skipped; future TeamInbox updates will be unread"
    }))
}

fn run_team_inbox_channels_at(
    root: impl AsRef<Path>,
    input: &TeamInboxChannelsInput,
) -> Result<String, ToolError> {
    if let Some(consumer_id) = input.consumer_id.as_deref() {
        reject_reserved_team_inbox_consumer(consumer_id)?;
    }
    let store = TeamInboxStore::open_read_only_at(root);
    let channels = store
        .list_channels(input.consumer_id.as_deref())
        .map_err(map_team_inbox_error)?;
    let channel_values = channels
        .iter()
        .map(|channel| channel_to_tool_json(channel, input.consumer_id.is_some()))
        .collect::<Vec<_>>();
    to_pretty_json(json!({
        "status": "ok",
        "store_mode": store_mode_label(store.mode()),
        "channels": channel_values
    }))
}

fn run_team_inbox_unread_at(
    root: impl AsRef<Path>,
    input: &TeamInboxUnreadInput,
) -> Result<String, ToolError> {
    reject_reserved_team_inbox_consumer(&input.consumer_id)?;
    let limit = normalize_team_inbox_limit(input.limit)?;
    let store = TeamInboxStore::open_read_only_at(root);
    let cursor = store
        .cursor(&input.consumer_id, &input.channel)
        .map_err(map_team_inbox_error)?;
    let updates = if cursor.is_some() {
        store
            .unread_updates(&input.consumer_id, &input.channel, limit)
            .map_err(map_team_inbox_error)?
    } else {
        Vec::new()
    };
    let update_values = updates
        .iter()
        .map(|update| update_to_unread_tool_json(&store, &input.consumer_id, update))
        .collect::<Result<Vec<_>, _>>()?;
    let next_ack_seq = updates.first().map(|update| update.seq);
    let joined = cursor.is_some();
    to_pretty_json(json!({
        "status": "ok",
        "store_mode": store_mode_label(store.mode()),
        "consumer_id": input.consumer_id,
        "channel": input.channel,
        "joined": joined,
        "last_seen_seq": cursor,
        "next_ack_seq": next_ack_seq,
        "count": update_values.len(),
        "updates": update_values,
        "guidance": if joined {
            Value::Null
        } else {
            json!(format!(
                "consumer {:?} is not joined to channel {:?}; call TeamInboxJoin first. TeamInboxJoin starts from the current tail, so existing backlog is skipped.",
                input.consumer_id, input.channel
            ))
        },
        "body_policy": "raw body text is not emitted; use body_ref/retrieve artifact flow if needed"
    }))
}

fn run_team_inbox_ack_at(
    root: impl AsRef<Path>,
    input: &TeamInboxAckInput,
) -> Result<String, ToolError> {
    reject_reserved_team_inbox_consumer(&input.consumer_id)?;
    let root = root.as_ref();
    let read_store = TeamInboxStore::open_read_only_at(root);
    let update = read_store
        .update(&input.update_id)
        .map_err(map_team_inbox_error)?
        .ok_or_else(|| ToolError::NotFound(format!("TeamInbox update not found: {}", input.update_id)))?;
    if update.channel != input.channel {
        return Err(ToolError::InvalidInput(format!(
            "update {} belongs to channel {:?}, not {:?}",
            input.update_id, update.channel, input.channel
        )));
    }
    let cursor = read_store
        .cursor(&input.consumer_id, &input.channel)
        .map_err(map_team_inbox_error)?;
    let Some(last_seen_seq) = cursor else {
        return Err(ToolError::InvalidInput(format!(
            "consumer_id {:?} is not joined to channel {:?}; cannot ack update seq {}",
            input.consumer_id, input.channel, update.seq
        )));
    };
    if update.seq <= last_seen_seq {
        return Err(ToolError::InvalidInput(format!(
            "consumer_id {:?} on channel {:?} has cursor last_seen_seq {}; update seq {} is already behind the cursor",
            input.consumer_id, input.channel, last_seen_seq, update.seq
        )));
    }

    let mut store = TeamInboxStore::open_at(root);
    let now = now_secs();
    store
        .mark_injected(&input.consumer_id, &input.update_id, &input.turn_id, now)
        .map_err(map_team_inbox_error)?;
    let advanced_to = store
        .ack_update(
            &input.consumer_id,
            &input.channel,
            &input.update_id,
            &input.turn_id,
            now,
        )
        .map_err(map_team_inbox_error)?;
    to_pretty_json(json!({
        "status": "acked",
        "store_mode": store_mode_label(store.mode()),
        "consumer_id": input.consumer_id,
        "channel": input.channel,
        "update_id": input.update_id,
        "turn_id": input.turn_id,
        "advanced_to_seq": advanced_to,
        "message": "Update marked injected and acked for the supplied turn_id"
    }))
}

fn run_team_inbox_leave_at(
    root: impl AsRef<Path>,
    input: &TeamInboxLeaveInput,
) -> Result<String, ToolError> {
    reject_reserved_team_inbox_consumer(&input.consumer_id)?;
    let mut store = TeamInboxStore::open_at(root);
    store
        .leave_channel(&input.consumer_id, &input.channel)
        .map_err(map_team_inbox_error)?;
    to_pretty_json(json!({
        "status": "left",
        "store_mode": store_mode_label(store.mode()),
        "consumer_id": input.consumer_id,
        "channel": input.channel,
        "message": "Consumer unsubscribed from TeamInbox channel; delivery history was kept"
    }))
}

fn team_inbox_store_root(ctx: &ToolContext) -> PathBuf {
    if let Some(path) = std::env::var_os("ZO_TEAM_INBOX_STORE") {
        return PathBuf::from(path);
    }
    ctx.workspace_root
        .as_ref()
        .or(ctx.cwd.as_ref())
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zo")
        .join("team_inbox")
}

fn reject_reserved_team_inbox_consumer(consumer_id: &str) -> Result<(), ToolError> {
    // Trim + case-fold so `session:abc` / ` session:abc` cannot masquerade as
    // (or be confused with) the runtime turn-digest consumer either.
    if consumer_id
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("session:")
    {
        return Err(ToolError::InvalidInput(
            "consumer_id prefix \"session:\" is reserved for the runtime turn digest".into(),
        ));
    }
    Ok(())
}

fn normalize_team_inbox_limit(limit: usize) -> Result<usize, ToolError> {
    if (1..=100).contains(&limit) {
        Ok(limit)
    } else {
        Err(ToolError::InvalidInput(
            "TeamInboxUnread limit must be between 1 and 100".into(),
        ))
    }
}

fn generated_team_update_id(input: &TeamInboxPostInput) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.channel.as_bytes());
    hasher.update([0]);
    hasher.update(input.source.as_bytes());
    hasher.update([0]);
    hasher.update(input.summary.as_bytes());
    hasher.update([0]);
    if let Some(body) = &input.body {
        hasher.update(body.as_bytes());
    }
    hasher.update([0]);
    if let Some(task_id) = &input.task_id {
        hasher.update(task_id.as_bytes());
    }
    hasher.update([0]);
    if let Some(status) = &input.status {
        hasher.update(status.as_bytes());
    }
    hasher.update([0]);
    hasher.update(format!("{}", current_nanos()).as_bytes());

    let digest = hasher.finalize();
    let mut suffix = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(suffix, "{byte:02x}");
    }
    format!("teaminbox_{suffix}")
}

fn current_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn update_to_tool_json(update: &TeamUpdate) -> Value {
    json!({
        "seq": update.seq,
        "id": update.id,
        "channel": update.channel,
        "source": update.source,
        "created_at_unix": update.created_at_unix,
        "priority": update.priority,
        "summary": update.summary,
        "body_ref": update.body_ref.as_ref().map(|body_ref| {
            json!({
                "sha256": body_ref.sha256,
                "size_bytes": body_ref.size_bytes,
                "kind": body_ref.kind,
            })
        }),
        "task_id": update.task_id,
        "status": update.status,
    })
}

fn channel_to_tool_json(channel: &TeamInboxChannel, include_consumer: bool) -> Value {
    let mut value = json!({
        "channel": channel.channel,
        "update_count": channel.update_count,
        "last_seq": channel.last_seq,
        "last_created_at_unix": channel.last_created_at_unix,
    });
    if include_consumer {
        if let Some(object) = value.as_object_mut() {
            object.insert("joined".to_string(), json!(channel.cursor_seq.is_some()));
            if let Some(cursor_seq) = channel.cursor_seq {
                object.insert("cursor_seq".to_string(), json!(cursor_seq));
            }
        }
    }
    value
}

fn update_to_unread_tool_json(
    store: &TeamInboxStore,
    consumer_id: &str,
    update: &TeamUpdate,
) -> Result<Value, ToolError> {
    let delivery = store
        .delivery(consumer_id, &update.id)
        .map_err(map_team_inbox_error)?;
    let mut value = update_to_tool_json(update);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "delivery_state".to_string(),
            json!(delivery
                .as_ref()
                .map_or("none", |record| delivery_state_label(record.state))),
        );
        object.insert(
            "retry_count".to_string(),
            json!(delivery.as_ref().map_or(0, |record| record.retry_count)),
        );
    }
    Ok(value)
}

fn delivery_state_label(state: TeamInboxDeliveryState) -> &'static str {
    match state {
        TeamInboxDeliveryState::Pending => "none",
        TeamInboxDeliveryState::Injected => "injected",
        TeamInboxDeliveryState::Acked => "acked",
        TeamInboxDeliveryState::Failed => "failed",
        TeamInboxDeliveryState::Stale => "stale",
    }
}

fn store_mode_label(mode: StoreMode) -> &'static str {
    match mode {
        StoreMode::ReadWrite => "read_write",
        StoreMode::ReadOnly => "read_only",
    }
}

fn map_team_inbox_error(error: TeamInboxStoreError) -> ToolError {
    match error {
        TeamInboxStoreError::InvalidInput(message) => ToolError::InvalidInput(message),
        other => ToolError::Execution(other.to_string()),
    }
}

fn run_team_create(
    teams: &TeamRegistry,
    tasks: &TaskRegistry,
    input: TeamCreateInput,
) -> Result<String, ToolError> {
    let task_ids = resolve_team_task_ids(tasks, input.tasks)?;
    let team = teams
        .create(&input.name, task_ids)
        .map_err(ToolError::Execution)?;
    for task_id in &team.task_ids {
        tasks
            .assign_team(task_id, &team.team_id)
            .map_err(ToolError::Execution)?;
    }
    to_pretty_json(json!({
        "team_id": team.team_id,
        "name": team.name,
        "task_count": team.task_ids.len(),
        "task_ids": team.task_ids,
        "status": team.status,
        "created_at": team.created_at,
        "execution_status": "not_wired",
        "message": "Team record created; task execution is not wired"
    }))
}

fn run_team_delete(
    teams: &TeamRegistry,
    tasks: &TaskRegistry,
    input: &TeamDeleteInput,
) -> Result<String, ToolError> {
    let team = teams
        .get(&input.team_id)
        .ok_or_else(|| ToolError::NotFound(format!("team not found: {}", input.team_id)))?;

    let mut active_task_ids = Vec::new();
    let mut terminal_task_ids = Vec::new();

    for task_id in &team.task_ids {
        let task = tasks.get(task_id).ok_or_else(|| {
            ToolError::Execution(format!(
                "team {} references missing task {task_id}",
                team.team_id
            ))
        })?;
        match task.status {
            TaskStatus::Created | TaskStatus::Running => active_task_ids.push(task.task_id),
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Stopped => {
                terminal_task_ids.push(task.task_id);
            }
        }
    }

    let mut stopped_task_ids = Vec::with_capacity(active_task_ids.len());
    for task_id in active_task_ids {
        tasks.stop(&task_id).map_err(ToolError::Execution)?;
        stopped_task_ids.push(task_id);
    }

    let deleted = teams.delete(&input.team_id).map_err(ToolError::Execution)?;
    to_pretty_json(json!({
        "team_id": deleted.team_id,
        "name": deleted.name,
        "status": deleted.status,
        "stopped_task_ids": stopped_task_ids,
        "already_terminal_task_ids": terminal_task_ids,
        "message": "Team deleted"
    }))
}

// The session that owns this working directory runs due crons from its one
// idle-time autonomy driver (`zo-ide`'s `autonomy::crons`, t-2901): when the
// session is idle and a cron's local minute has come, a turn opens with the
// cron's prompt and the run is recorded through this registry's transaction.
// `CronRunDue` stays as the by-hand path and sees such a run as already
// recorded. The receipt names that engine so the model does not wait for a
// turn from somewhere else.
const CRON_AUTOMATIC_SCHEDULER_STATUS: &str = AUTOMATIC_SCHEDULER_STATUS;
const CRON_AUTOMATIC_SCHEDULER_NOTE: &str = AUTOMATIC_SCHEDULER_NOTE;

fn run_cron_create(crons: &CronRegistry, input: &CronCreateInput) -> Result<String, ToolError> {
    validate_cron_schedule(&input.schedule).map_err(ToolError::InvalidInput)?;
    if input.prompt.trim().is_empty() {
        return Err(ToolError::InvalidInput(
            "cron prompt must not be empty".to_string(),
        ));
    }

    let entry = crons
        .create(&input.schedule, &input.prompt, input.description.as_deref())
        .map_err(ToolError::Execution)?;
    let next_due_at = crons
        .next_due_at(&entry.cron_id, now_secs())
        .map_err(ToolError::Execution)?;
    let zone = cron_zone();
    to_pretty_json(json!({
        "cron_id": entry.cron_id,
        "schedule": entry.schedule,
        "prompt": entry.prompt,
        "description": entry.description,
        "enabled": entry.enabled,
        "created_at": entry.created_at,
        "updated_at": entry.updated_at,
        "last_run_at": entry.last_run_at,
        "run_count": entry.run_count,
        "next_due_at": next_due_at,
        "time_zone": zone.label,
        "time_zone_utc_offset_secs": zone.offset_secs,
        "scheduler_status": "manual_run_due",
        "automatic_scheduler_status": CRON_AUTOMATIC_SCHEDULER_STATUS,
        "automatic_scheduler_note": CRON_AUTOMATIC_SCHEDULER_NOTE
    }))
}

fn run_cron_delete(crons: &CronRegistry, input: &CronDeleteInput) -> Result<String, ToolError> {
    match crons.delete(&input.cron_id) {
        Ok(entry) => to_pretty_json(json!({
            "cron_id": entry.cron_id,
            "schedule": entry.schedule,
            "status": "deleted",
            "message": "Cron entry permanently removed"
        })),
        Err(error) => Err(ToolError::Execution(error)),
    }
}

fn run_cron_list(crons: &CronRegistry) -> Result<String, ToolError> {
    let now = now_secs();
    let entries: Vec<_> = crons
        .list(false)
        .into_iter()
        .map(|entry| {
            let next_due_at = crons.next_due_at(&entry.cron_id, now).ok().flatten();
            json!({
                "cron_id": entry.cron_id,
                "schedule": entry.schedule,
                "prompt": entry.prompt,
                "description": entry.description,
                "enabled": entry.enabled,
                "run_count": entry.run_count,
                "last_run_at": entry.last_run_at,
                "created_at": entry.created_at,
                "updated_at": entry.updated_at,
                "next_due_at": next_due_at,
                "scheduler_status": "manual_run_due",
                "automatic_scheduler_status": CRON_AUTOMATIC_SCHEDULER_STATUS,
                "automatic_scheduler_note": CRON_AUTOMATIC_SCHEDULER_NOTE
            })
        })
        .collect();
    let count = entries.len();
    let zone = cron_zone();
    to_pretty_json(json!({
        "crons": entries,
        "count": count,
        "time_zone": zone.label,
        "time_zone_utc_offset_secs": zone.offset_secs
    }))
}

fn run_cron_run_due(
    crons: &CronRegistry,
    tasks: &TaskRegistry,
    input: &CronRunDueInput,
) -> Result<String, ToolError> {
    run_cron_run_due_at(crons, tasks, input, now_secs())
}

fn run_cron_run_due_at(
    crons: &CronRegistry,
    tasks: &TaskRegistry,
    input: &CronRunDueInput,
    now: u64,
) -> Result<String, ToolError> {
    let all = crons.list(false);
    let enabled_count = all.iter().filter(|entry| entry.enabled).count();
    let due = crons.due_at(now);
    let mut runs = Vec::with_capacity(due.len());
    let mut created_task_ids = Vec::new();

    for due_entry in due {
        let cron_id = due_entry.entry.cron_id.clone();
        if input.dry_run {
            runs.push(json!({
                "cron_id": cron_id,
                "due_at": due_entry.due_at,
                "next_due_at_after_run": due_entry.next_due_at,
                "status": "due",
                "task_id": null
            }));
            continue;
        }

        let description = format!(
            "Cron {} due at {}{}",
            cron_id,
            due_entry.due_at,
            due_entry
                .entry
                .description
                .as_deref()
                .map(|description| format!(" — {description}"))
                .unwrap_or_default()
        );
        let task = tasks.create(&due_entry.entry.prompt, Some(&description));
        let recorded = crons
            .record_due_run_at(&cron_id, due_entry.due_at)
            .map_err(|error| {
                let _ = tasks.remove(&task.task_id);
                ToolError::Execution(error)
            })?;
        if !recorded {
            let _ = tasks.remove(&task.task_id);
            runs.push(json!({
                "cron_id": cron_id,
                "due_at": due_entry.due_at,
                "next_due_at_after_run": due_entry.next_due_at,
                "status": "already_recorded",
                "task_id": null
            }));
            continue;
        }
        created_task_ids.push(task.task_id.clone());
        runs.push(json!({
            "cron_id": cron_id,
            "due_at": due_entry.due_at,
            "next_due_at_after_run": due_entry.next_due_at,
            "status": "enqueued",
            "task_id": task.task_id,
            "task_status": task.status
        }));
    }

    let due_count = runs.len();
    to_pretty_json(json!({
        "checked_at": now,
        "dry_run": input.dry_run,
        "scheduler_status": "manual_run_due",
        "automatic_scheduler_status": CRON_AUTOMATIC_SCHEDULER_STATUS,
        "automatic_scheduler_note": CRON_AUTOMATIC_SCHEDULER_NOTE,
        "total_crons": all.len(),
        "enabled_crons": enabled_count,
        "due_count": due_count,
        "skipped_disabled": all.len().saturating_sub(enabled_count),
        "skipped_not_due": enabled_count.saturating_sub(due_count),
        "created_task_ids": created_task_ids,
        "runs": runs,
        "message": if input.dry_run {
            "Due cron records previewed; no tasks were created"
        } else {
            "Due cron records were enqueued as Created task records; no agent execution was started"
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_inbox_store::{append_event_to, TeamInboxEvent};
    use serde_json::Value;

    fn parse_json(output: &str) -> Value {
        serde_json::from_str(output).expect("valid json output")
    }

    fn temp_team_inbox_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "zo-team-tools-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    /// The host-side seams (`ensure_team_inbox_store` + `host_post_team_inbox_update`)
    /// used by the autonomous-loop digest path: the store is created on demand and
    /// a plain post lands in the target channel, readable by an ordinary consumer.
    #[test]
    fn host_post_creates_store_and_records_update() {
        let dir = temp_team_inbox_dir("host-post");
        // Ensure-store creates the SQLite store on an empty directory.
        ensure_team_inbox_store(&dir).expect("store is created on demand");
        // A host post lands in the digest channel.
        host_post_team_inbox_update(&dir, "digest", "zo-loop", "loop-1 paused: awaiting decision")
            .expect("host post succeeds");

        // A non-session consumer joined from now sees only later posts, so read the
        // channel from seq 0 by joining before a second post to prove the write path.
        run_team_inbox_join_at(
            &dir,
            &TeamInboxJoinInput {
                consumer_id: "reader-1".into(),
                channel: "digest".into(),
            },
        )
        .expect("join");
        host_post_team_inbox_update(&dir, "digest", "zo-loop", "second note")
            .expect("second host post");
        let unread = run_team_inbox_unread_at(
            &dir,
            &TeamInboxUnreadInput {
                consumer_id: "reader-1".into(),
                channel: "digest".into(),
                limit: 10,
            },
        )
        .expect("unread");
        let unread_json = parse_json(&unread);
        assert_eq!(unread_json["count"], 1, "the post after the join is unread");
        assert_eq!(unread_json["updates"][0]["summary"], "second note");
        assert_eq!(unread_json["updates"][0]["source"], "zo-loop");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn team_inbox_manual_lifecycle_hides_raw_body_and_acknowledges_update() {
        let dir = temp_team_inbox_dir("lifecycle");
        let join = run_team_inbox_join_at(
            &dir,
            &TeamInboxJoinInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
            },
        )
        .expect("join");
        assert_eq!(parse_json(&join)["status"], "joined_from_now");

        let post = run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("update-1".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "found flaky test".into(),
                body: Some("secret raw body details".into()),
                priority: Some(TeamInboxPriority::High),
                task_id: Some("task-1".into()),
                status: Some("found".into()),
            },
        )
        .expect("post");
        let post_json = parse_json(&post);
        assert_eq!(post_json["status"], "posted");
        assert_eq!(post_json["update"]["id"], "update-1");
        assert!(post_json["update"]["body_ref"]["sha256"].is_string());

        let unread = run_team_inbox_unread_at(
            &dir,
            &TeamInboxUnreadInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                limit: 10,
            },
        )
        .expect("unread");
        assert!(!unread.contains("secret raw body details"));
        let unread_json = parse_json(&unread);
        assert_eq!(unread_json["count"], 1);
        assert_eq!(unread_json["updates"][0]["summary"], "found flaky test");
        assert!(unread_json["updates"][0]["body_ref"].get("preview").is_none());

        let ack = run_team_inbox_ack_at(
            &dir,
            &TeamInboxAckInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                update_id: "update-1".into(),
                turn_id: "turn-1".into(),
            },
        )
        .expect("ack");
        assert_eq!(parse_json(&ack)["status"], "acked");

        let unread_after_ack = run_team_inbox_unread_at(
            &dir,
            &TeamInboxUnreadInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                limit: 10,
            },
        )
        .expect("unread after ack");
        assert_eq!(parse_json(&unread_after_ack)["count"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_unread_reports_joined_false_guidance_without_creating_files() {
        let dir = temp_team_inbox_dir("unjoined-guidance");
        let unread = run_team_inbox_unread_at(
            &dir,
            &TeamInboxUnreadInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                limit: 10,
            },
        )
        .expect("unread");
        let json = parse_json(&unread);
        assert_eq!(json["joined"], false);
        assert_eq!(json["last_seen_seq"], Value::Null);
        assert_eq!(json["next_ack_seq"], Value::Null);
        assert_eq!(json["count"], 0);
        assert!(json["guidance"]
            .as_str()
            .expect("guidance")
            .contains("TeamInboxJoin"));
        assert!(!dir.exists(), "read-only unread must not create store directory");
    }

    #[test]
    fn team_inbox_unread_reports_cursor_delivery_state_and_next_ack_seq() {
        let dir = temp_team_inbox_dir("unread-cursor-delivery");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("backlog".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "old backlog".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post backlog");
        let join = parse_json(
            &run_team_inbox_join_at(
                &dir,
                &TeamInboxJoinInput {
                    consumer_id: "session-1".into(),
                    channel: "ci".into(),
                },
            )
            .expect("join"),
        );
        assert_eq!(join["last_seen_seq"], 1);
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("update-1".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "new update".into(),
                body: Some("do not leak this raw body".into()),
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post update");

        let unread = run_team_inbox_unread_at(
            &dir,
            &TeamInboxUnreadInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                limit: 10,
            },
        )
        .expect("unread");
        assert!(!unread.contains("do not leak this raw body"));
        let unread_json = parse_json(&unread);
        assert_eq!(unread_json["joined"], true);
        assert_eq!(unread_json["last_seen_seq"], 1);
        assert_eq!(unread_json["next_ack_seq"], 2);
        assert_eq!(unread_json["count"], 1);
        assert_eq!(unread_json["updates"][0]["seq"], 2);
        assert_eq!(unread_json["updates"][0]["delivery_state"], "none");
        assert_eq!(unread_json["updates"][0]["retry_count"], 0);

        let ack = parse_json(
            &run_team_inbox_ack_at(
                &dir,
                &TeamInboxAckInput {
                    consumer_id: "session-1".into(),
                    channel: "ci".into(),
                    update_id: "update-1".into(),
                    turn_id: "turn-1".into(),
                },
            )
            .expect("ack"),
        );
        assert_eq!(ack["advanced_to_seq"], 2);

        let unread_after_ack = parse_json(
            &run_team_inbox_unread_at(
                &dir,
                &TeamInboxUnreadInput {
                    consumer_id: "session-1".into(),
                    channel: "ci".into(),
                    limit: 10,
                },
            )
            .expect("unread after ack"),
        );
        assert_eq!(unread_after_ack["joined"], true);
        assert_eq!(unread_after_ack["last_seen_seq"], 2);
        assert_eq!(unread_after_ack["next_ack_seq"], Value::Null);
        assert_eq!(unread_after_ack["count"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_unread_reports_injected_delivery_state() {
        let dir = temp_team_inbox_dir("unread-injected");
        run_team_inbox_join_at(
            &dir,
            &TeamInboxJoinInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
            },
        )
        .expect("join");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("update-1".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "new update".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post update");
        let mut store = TeamInboxStore::open_at(&dir);
        store
            .mark_injected("session-1", "update-1", "turn-1", 42)
            .expect("mark injected");

        let unread = parse_json(
            &run_team_inbox_unread_at(
                &dir,
                &TeamInboxUnreadInput {
                    consumer_id: "session-1".into(),
                    channel: "ci".into(),
                    limit: 10,
                },
            )
            .expect("unread"),
        );
        assert_eq!(unread["updates"][0]["delivery_state"], "injected");
        assert_eq!(unread["updates"][0]["retry_count"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_unread_read_only_jsonl_reports_delivery_metadata() {
        let dir = temp_team_inbox_dir("readonly-unread-delivery");
        std::fs::create_dir_all(dir.join("team_inbox.sqlite3"))
            .expect("db path directory forces open failure");
        let update = TeamUpdate {
            seq: 7,
            id: "from-jsonl".into(),
            channel: "ci".into(),
            source: "agent:reviewer".into(),
            created_at_unix: 1,
            priority: TeamInboxPriority::High,
            summary: "jsonl summary".into(),
            body_ref: None,
            task_id: None,
            status: None,
        };
        append_event_to(
            &dir.join("team_inbox.jsonl"),
            &TeamInboxEvent::Post { update },
        )
        .expect("post event");
        append_event_to(
            &dir.join("team_inbox.jsonl"),
            &TeamInboxEvent::CursorAdvance {
                consumer_id: "s".into(),
                channel: "ci".into(),
                last_seen_seq: 0,
            },
        )
        .expect("cursor event");
        append_event_to(
            &dir.join("team_inbox.jsonl"),
            &TeamInboxEvent::Delivery {
                update_id: "from-jsonl".into(),
                consumer_id: "s".into(),
                state: TeamInboxDeliveryState::Failed,
                turn_id: Some("turn-1".into()),
                retry_count: 2,
                updated_at_unix: 2,
            },
        )
        .expect("delivery event");

        let unread = parse_json(
            &run_team_inbox_unread_at(
                &dir,
                &TeamInboxUnreadInput {
                    consumer_id: "s".into(),
                    channel: "ci".into(),
                    limit: 10,
                },
            )
            .expect("unread"),
        );
        assert_eq!(unread["store_mode"], "read_only");
        assert_eq!(unread["joined"], true);
        assert_eq!(unread["last_seen_seq"], 0);
        assert_eq!(unread["next_ack_seq"], 7);
        assert_eq!(unread["count"], 1);
        assert_eq!(unread["updates"][0]["delivery_state"], "failed");
        assert_eq!(unread["updates"][0]["retry_count"], 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_unread_reports_failed_delivery_retry_count() {
        let dir = temp_team_inbox_dir("unread-failed-retry");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("backlog".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "old backlog".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post backlog");
        run_team_inbox_join_at(
            &dir,
            &TeamInboxJoinInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
            },
        )
        .expect("join");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("update-1".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "new update".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post update");
        let mut store = TeamInboxStore::open_at(&dir);
        store
            .mark_injected("session-1", "update-1", "turn-1", 41)
            .expect("mark injected");
        let state = store
            .record_failure("session-1", "ci", "update-1", "turn-1", 42, 3)
            .expect("record failure");
        assert_eq!(state, TeamInboxDeliveryState::Failed);

        let unread = parse_json(
            &run_team_inbox_unread_at(
                &dir,
                &TeamInboxUnreadInput {
                    consumer_id: "session-1".into(),
                    channel: "ci".into(),
                    limit: 10,
                },
            )
            .expect("unread"),
        );
        assert_eq!(unread["count"], 1);
        assert_eq!(unread["updates"][0]["id"], "update-1");
        assert_eq!(unread["updates"][0]["delivery_state"], "failed");
        assert_eq!(unread["updates"][0]["retry_count"], 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_ack_reports_clear_cursor_errors() {
        let dir = temp_team_inbox_dir("ack-cursor-errors");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("update-1".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "new update".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post update");
        let not_joined = run_team_inbox_ack_at(
            &dir,
            &TeamInboxAckInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                update_id: "update-1".into(),
                turn_id: "turn-1".into(),
            },
        )
        .expect_err("ack without join should reject");
        assert!(matches!(not_joined, ToolError::InvalidInput(_)));
        assert!(not_joined.to_string().contains("not joined"));
        assert!(not_joined.to_string().contains("session-1"));
        assert!(not_joined.to_string().contains("ci"));
        assert!(not_joined.to_string().contains("seq 1"));

        run_team_inbox_join_at(
            &dir,
            &TeamInboxJoinInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
            },
        )
        .expect("join after update");
        let behind_cursor = run_team_inbox_ack_at(
            &dir,
            &TeamInboxAckInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                update_id: "update-1".into(),
                turn_id: "turn-1".into(),
            },
        )
        .expect_err("ack behind cursor should reject");
        assert!(matches!(behind_cursor, ToolError::InvalidInput(_)));
        assert!(behind_cursor.to_string().contains("last_seen_seq 1"));
        assert!(behind_cursor.to_string().contains("update seq 1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_ack_rejects_wrong_channel_without_inject_side_effect() {
        let dir = temp_team_inbox_dir("wrong-channel");
        run_team_inbox_join_at(
            &dir,
            &TeamInboxJoinInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
            },
        )
        .expect("join");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("review-update".into()),
                channel: "review".into(),
                source: "agent:reviewer".into(),
                summary: "wrong channel".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post");

        let error = run_team_inbox_ack_at(
            &dir,
            &TeamInboxAckInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                update_id: "review-update".into(),
                turn_id: "turn-1".into(),
            },
        )
        .expect_err("wrong channel should reject");
        assert!(matches!(error, ToolError::InvalidInput(_)));

        let store = TeamInboxStore::open_at(&dir);
        let delivery = store
            .delivery("session-1", "review-update")
            .expect("delivery query");
        assert!(delivery.is_none(), "wrong-channel ack must not mark injected");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_ack_unknown_update_does_not_create_store_files() {
        let dir = temp_team_inbox_dir("ack-missing");
        let error = run_team_inbox_ack_at(
            &dir,
            &TeamInboxAckInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                update_id: "missing".into(),
                turn_id: "turn-1".into(),
            },
        )
        .expect_err("missing update should reject");
        assert!(matches!(error, ToolError::NotFound(_)));
        assert!(!dir.exists(), "unknown ack must not create store directory");
    }

    #[test]
    fn team_inbox_unread_on_clean_store_does_not_create_files() {
        let dir = temp_team_inbox_dir("clean-read");
        let unread = run_team_inbox_unread_at(
            &dir,
            &TeamInboxUnreadInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                limit: 10,
            },
        )
        .expect("clean unread");
        assert_eq!(parse_json(&unread)["count"], 0);
        assert!(!dir.exists(), "read-only unread must not create store directory");
    }

    #[test]
    fn team_inbox_unread_rejects_out_of_range_limit() {
        let dir = temp_team_inbox_dir("limit");
        let error = run_team_inbox_unread_at(
            &dir,
            &TeamInboxUnreadInput {
                consumer_id: "session-1".into(),
                channel: "ci".into(),
                limit: 0,
            },
        )
        .expect_err("zero limit rejected");
        assert!(matches!(error, ToolError::InvalidInput(_)));
        let _ = std::fs::remove_dir_all(dir);
    }


    #[test]
    fn team_inbox_channels_lists_channels_without_raw_body_and_no_consumer_cursor_keys() {
        let dir = temp_team_inbox_dir("channels-list");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("ci-1".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "ci summary".into(),
                body: Some("raw body must not leak".into()),
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post ci");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("review-1".into()),
                channel: "review".into(),
                source: "agent:reviewer".into(),
                summary: "review summary".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post review");

        let output = run_team_inbox_channels_at(&dir, &TeamInboxChannelsInput::default())
            .expect("channels");
        assert!(!output.contains("raw body must not leak"));
        let json = parse_json(&output);
        assert_eq!(json["status"], "ok");
        assert_eq!(json["store_mode"], "read_only");
        let channels = json["channels"].as_array().expect("channels array");
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0]["channel"], "ci");
        assert_eq!(channels[0]["update_count"], 1);
        assert_eq!(channels[0]["last_seq"], 1);
        assert!(channels[0]["last_created_at_unix"].as_u64().is_some());
        assert!(channels[0].get("joined").is_none());
        assert!(channels[0].get("cursor_seq").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_channels_reports_consumer_join_state_and_cursor() {
        let dir = temp_team_inbox_dir("channels-consumer");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("ci-1".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "ci summary".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post ci");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("review-1".into()),
                channel: "review".into(),
                source: "agent:reviewer".into(),
                summary: "review summary".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post review");
        run_team_inbox_join_at(
            &dir,
            &TeamInboxJoinInput {
                consumer_id: "reader".into(),
                channel: "ci".into(),
            },
        )
        .expect("join ci");

        let json = parse_json(
            &run_team_inbox_channels_at(
                &dir,
                &TeamInboxChannelsInput {
                    consumer_id: Some("reader".into()),
                },
            )
            .expect("channels"),
        );
        let channels = json["channels"].as_array().expect("channels array");
        assert_eq!(channels[0]["channel"], "ci");
        assert_eq!(channels[0]["joined"], true);
        assert_eq!(channels[0]["cursor_seq"], 1);
        assert_eq!(channels[1]["channel"], "review");
        assert_eq!(channels[1]["joined"], false);
        assert!(channels[1].get("cursor_seq").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_channels_on_clean_store_does_not_create_files() {
        let dir = temp_team_inbox_dir("channels-clean-read");
        let channels = run_team_inbox_channels_at(&dir, &TeamInboxChannelsInput::default())
            .expect("clean channels");
        assert_eq!(parse_json(&channels)["channels"].as_array().unwrap().len(), 0);
        assert!(!dir.exists(), "read-only channels must not create store directory");
    }

    #[test]
    fn team_inbox_leave_unsubscribes_but_keeps_delivery_history() {
        let dir = temp_team_inbox_dir("leave");
        run_team_inbox_join_at(
            &dir,
            &TeamInboxJoinInput {
                consumer_id: "reader".into(),
                channel: "ci".into(),
            },
        )
        .expect("join");
        run_team_inbox_post_at(
            &dir,
            TeamInboxPostInput {
                id: Some("update-1".into()),
                channel: "ci".into(),
                source: "agent:reviewer".into(),
                summary: "new update".into(),
                body: None,
                priority: None,
                task_id: None,
                status: None,
            },
        )
        .expect("post");
        let mut store = TeamInboxStore::open_at(&dir);
        store
            .mark_injected("reader", "update-1", "turn-1", 42)
            .expect("mark injected");
        drop(store);

        let left = parse_json(
            &run_team_inbox_leave_at(
                &dir,
                &TeamInboxLeaveInput {
                    consumer_id: "reader".into(),
                    channel: "ci".into(),
                },
            )
            .expect("leave"),
        );
        assert_eq!(left["status"], "left");
        let store = TeamInboxStore::open_at(&dir);
        assert_eq!(store.cursor("reader", "ci").expect("cursor"), None);
        assert_eq!(
            store
                .delivery("reader", "update-1")
                .expect("delivery")
                .expect("delivery kept")
                .state,
            TeamInboxDeliveryState::Injected
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn team_inbox_leave_reports_clear_not_joined_error() {
        let dir = temp_team_inbox_dir("leave-not-joined");
        let error = run_team_inbox_leave_at(
            &dir,
            &TeamInboxLeaveInput {
                consumer_id: "reader".into(),
                channel: "ci".into(),
            },
        )
        .expect_err("leave without join should reject");
        assert!(matches!(error, ToolError::InvalidInput(_)));
        assert!(error.to_string().contains("not joined"));
        assert!(error.to_string().contains("reader"));
        assert!(error.to_string().contains("ci"));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn assert_reserved_consumer_error(error: &ToolError) {
        assert!(matches!(error, ToolError::InvalidInput(_)));
        let message = error.to_string();
        assert!(message.contains("consumer_id"));
        assert!(message.contains("session:"));
        assert!(message.contains("reserved for the runtime turn digest"));
    }

    #[test]
    fn team_inbox_join_rejects_reserved_session_consumer_prefix() {
        let dir = temp_team_inbox_dir("reserved-join");
        assert_reserved_consumer_error(
            &run_team_inbox_join_at(
                &dir,
                &TeamInboxJoinInput {
                    consumer_id: "session:abc".into(),
                    channel: "ci".into(),
                },
            )
            .expect_err("reserved consumer should reject"),
        );
        assert!(!dir.exists(), "reserved join must not create store directory");
    }

    #[test]
    fn team_inbox_unread_rejects_reserved_session_consumer_prefix() {
        let dir = temp_team_inbox_dir("reserved-unread");
        assert_reserved_consumer_error(
            &run_team_inbox_unread_at(
                &dir,
                &TeamInboxUnreadInput {
                    consumer_id: "session:abc".into(),
                    channel: "ci".into(),
                    limit: 10,
                },
            )
            .expect_err("reserved consumer should reject"),
        );
        assert!(!dir.exists(), "reserved unread must not create store directory");
    }

    #[test]
    fn team_inbox_ack_rejects_reserved_session_consumer_prefix() {
        let dir = temp_team_inbox_dir("reserved-ack");
        assert_reserved_consumer_error(
            &run_team_inbox_ack_at(
                &dir,
                &TeamInboxAckInput {
                    consumer_id: "session:abc".into(),
                    channel: "ci".into(),
                    update_id: "update-1".into(),
                    turn_id: "turn-1".into(),
                },
            )
            .expect_err("reserved consumer should reject"),
        );
        assert!(!dir.exists(), "reserved ack must not create store directory");
    }

    #[test]
    fn team_inbox_leave_rejects_reserved_session_consumer_prefix() {
        let dir = temp_team_inbox_dir("reserved-leave");
        assert_reserved_consumer_error(
            &run_team_inbox_leave_at(
                &dir,
                &TeamInboxLeaveInput {
                    consumer_id: "session:abc".into(),
                    channel: "ci".into(),
                },
            )
            .expect_err("reserved consumer should reject"),
        );
        assert!(!dir.exists(), "reserved leave must not create store directory");
    }

    #[test]
    fn team_inbox_reserved_prefix_guard_rejects_case_and_whitespace_variants() {
        let dir = temp_team_inbox_dir("reserved-variants");
        for consumer in ["session:abc", "Session:abc", "  session:abc"] {
            assert_reserved_consumer_error(
                &run_team_inbox_join_at(
                    &dir,
                    &TeamInboxJoinInput {
                        consumer_id: consumer.into(),
                        channel: "ci".into(),
                    },
                )
                .expect_err("reserved consumer variant should reject"),
            );
        }
        assert!(
            !dir.exists(),
            "reserved variants must not create store directory"
        );
    }

    #[test]
    fn team_inbox_tool_specs_are_registered_with_expected_permissions() {
        let specs = tool_specs();
        let by_name = |name: &str| {
            specs
                .iter()
                .find(|spec| spec.name == name)
                .unwrap_or_else(|| panic!("missing spec {name}"))
        };
        assert_eq!(
            by_name("TeamInboxChannels").required_permission,
            PermissionMode::ReadOnly
        );
        assert_eq!(
            by_name("TeamInboxUnread").required_permission,
            PermissionMode::ReadOnly
        );
        assert_eq!(
            by_name("TeamInboxPost").required_permission,
            PermissionMode::WorkspaceWrite
        );
        assert_eq!(
            by_name("TeamInboxJoin").required_permission,
            PermissionMode::WorkspaceWrite
        );
        assert_eq!(
            by_name("TeamInboxAck").required_permission,
            PermissionMode::WorkspaceWrite
        );
        assert_eq!(
            by_name("TeamInboxLeave").required_permission,
            PermissionMode::WorkspaceWrite
        );
    }

    #[test]
    fn team_create_assigns_existing_and_inline_tasks() {
        let teams = TeamRegistry::new_in_memory();
        let tasks = TaskRegistry::new_in_memory();
        let existing = tasks.create("existing task", Some("already created"));

        let output = run_team_create(
            &teams,
            &tasks,
            TeamCreateInput {
                name: "parallel".to_string(),
                tasks: vec![
                    TeamTaskInput::ExistingTask {
                        task_id: existing.task_id.clone(),
                    },
                    TeamTaskInput::InlineTask {
                        prompt: "inline task".to_string(),
                        description: Some("generated".to_string()),
                    },
                ],
            },
        )
        .expect("team create should succeed");

        let json = parse_json(&output);
        let task_ids = json["task_ids"].as_array().expect("task id array");
        assert_eq!(json["task_count"], 2);

        let existing_task = tasks
            .get(&existing.task_id)
            .expect("existing task should still exist");
        assert_eq!(existing_task.team_id.as_deref(), json["team_id"].as_str(),);

        let inline_task_id = task_ids[1].as_str().expect("inline task id");
        let inline_task = tasks.get(inline_task_id).expect("inline task should exist");
        assert_eq!(inline_task.prompt, "inline task");
        assert_eq!(inline_task.team_id.as_deref(), json["team_id"].as_str());
    }

    #[test]
    fn team_create_rejects_empty_task_list() {
        let teams = TeamRegistry::new_in_memory();
        let tasks = TaskRegistry::new_in_memory();

        let error = run_team_create(
            &teams,
            &tasks,
            TeamCreateInput {
                name: "empty".to_string(),
                tasks: Vec::new(),
            },
        )
        .expect_err("empty team should be rejected");

        assert!(matches!(error, ToolError::InvalidInput(_)));
        assert!(error.to_string().contains("at least one task"));
    }

    #[test]
    fn team_create_rejects_duplicate_existing_tasks() {
        let teams = TeamRegistry::new_in_memory();
        let tasks = TaskRegistry::new_in_memory();
        let existing = tasks.create("existing task", None);

        let error = run_team_create(
            &teams,
            &tasks,
            TeamCreateInput {
                name: "duplicates".to_string(),
                tasks: vec![
                    TeamTaskInput::ExistingTask {
                        task_id: existing.task_id.clone(),
                    },
                    TeamTaskInput::ExistingTask {
                        task_id: existing.task_id.clone(),
                    },
                ],
            },
        )
        .expect_err("duplicate existing task refs should be rejected");

        assert!(matches!(error, ToolError::InvalidInput(_)));
        assert!(error.to_string().contains("duplicate task reference"));
    }

    #[test]
    fn team_create_reports_registry_capacity_errors() {
        let teams = TeamRegistry::new_in_memory();
        let tasks = TaskRegistry::new_in_memory();
        for i in 0..1024 {
            if teams.create(&format!("live {i}"), vec![]).is_err() {
                break;
            }
        }

        let error = run_team_create(
            &teams,
            &tasks,
            TeamCreateInput {
                name: "overflow".to_string(),
                tasks: vec![TeamTaskInput::InlineTask {
                    prompt: "overflow task".to_string(),
                    description: None,
                }],
            },
        )
        .expect_err("team cap should reject through the tool surface");

        assert!(matches!(error, ToolError::Execution(_)));
        assert!(error.to_string().contains("team registry is full"));
    }

    #[test]
    fn team_delete_stops_active_tasks_and_keeps_terminal_tasks() {
        let teams = TeamRegistry::new_in_memory();
        let tasks = TaskRegistry::new_in_memory();
        let active = tasks.create("active", None);
        let completed = tasks.create("completed", None);
        tasks
            .set_status(&completed.task_id, TaskStatus::Completed)
            .expect("set completed");

        let team = teams
            .create(
                "cleanup",
                vec![active.task_id.clone(), completed.task_id.clone()],
            )
            .expect("create cleanup team");
        tasks
            .assign_team(&active.task_id, &team.team_id)
            .expect("assign active");
        tasks
            .assign_team(&completed.task_id, &team.team_id)
            .expect("assign completed");

        let output = run_team_delete(
            &teams,
            &tasks,
            &TeamDeleteInput {
                team_id: team.team_id.clone(),
            },
        )
        .expect("team delete should succeed");

        let json = parse_json(&output);
        assert_eq!(json["status"], "deleted");
        assert_eq!(
            tasks.get(&active.task_id).expect("active task").status,
            TaskStatus::Stopped
        );
        assert_eq!(
            tasks
                .get(&completed.task_id)
                .expect("completed task")
                .status,
            TaskStatus::Completed
        );
        assert_eq!(
            json["stopped_task_ids"]
                .as_array()
                .expect("stopped ids")
                .len(),
            1
        );
        assert_eq!(
            json["already_terminal_task_ids"]
                .as_array()
                .expect("terminal ids")
                .len(),
            1
        );
    }

    #[test]
    fn team_delete_rejects_missing_referenced_tasks() {
        let teams = TeamRegistry::new_in_memory();
        let tasks = TaskRegistry::new_in_memory();
        let team = teams
            .create("broken", vec!["missing_task".to_string()])
            .expect("create broken team");

        let error = run_team_delete(
            &teams,
            &tasks,
            &TeamDeleteInput {
                team_id: team.team_id.clone(),
            },
        )
        .expect_err("broken team should fail deletion");

        assert!(matches!(error, ToolError::Execution(_)));
        assert!(error.to_string().contains("references missing task"));
        assert_eq!(
            teams.get(&team.team_id).expect("team still exists").status,
            runtime::team_cron_registry::TeamStatus::Created
        );
    }

    #[test]
    fn cron_create_rejects_invalid_schedule_and_empty_prompt() {
        let crons = CronRegistry::new_in_memory();

        let invalid_schedule = run_cron_create(
            &crons,
            &CronCreateInput {
                schedule: "@daily".to_string(),
                prompt: "daily check".to_string(),
                description: None,
            },
        )
        .expect_err("invalid schedule should fail");
        assert!(matches!(invalid_schedule, ToolError::InvalidInput(_)));

        let empty_prompt = run_cron_create(
            &crons,
            &CronCreateInput {
                schedule: "0 * * * *".to_string(),
                prompt: "  ".to_string(),
                description: None,
            },
        )
        .expect_err("empty prompt should fail");
        assert!(matches!(empty_prompt, ToolError::InvalidInput(_)));

        assert!(crons.is_empty());
    }

    #[test]
    fn cron_create_and_list_name_the_session_scheduler() {
        let crons = CronRegistry::new_in_memory();
        let output = run_cron_create(
            &crons,
            &CronCreateInput {
                schedule: "  */15 * * * *  ".to_string(),
                prompt: "  Check health  ".to_string(),
                description: Some("health".to_string()),
            },
        )
        .expect("cron create should succeed");

        let created = parse_json(&output);
        assert_eq!(created["schedule"], "*/15 * * * *");
        assert_eq!(created["prompt"], "Check health");
        assert_eq!(created["scheduler_status"], "manual_run_due");
        assert_eq!(created["automatic_scheduler_status"], "session_idle");
        assert!(created["next_due_at"].as_u64().is_some());
        assert_eq!(created["run_count"], 0);
        // The receipt says which wall clock it read the five fields against.
        let zone = cron_zone();
        assert_eq!(created["time_zone"], zone.label);
        assert_eq!(created["time_zone_utc_offset_secs"], zone.offset_secs);

        let list = parse_json(&run_cron_list(&crons).expect("cron list"));
        assert_eq!(list["count"], 1);
        let first = &list["crons"].as_array().expect("crons array")[0];
        assert_eq!(first["scheduler_status"], "manual_run_due");
        assert_eq!(first["automatic_scheduler_status"], "session_idle");
        assert!(first["next_due_at"].as_u64().is_some());
        assert!(first["updated_at"].as_u64().is_some());
        assert_eq!(list["time_zone"], zone.label);
        assert_eq!(list["time_zone_utc_offset_secs"], zone.offset_secs);
    }

    #[test]
    fn cron_run_due_previews_and_enqueues_due_tasks_idempotently() {
        // The fixtures below name a Monday 09:00 the way a person writes it, and
        // a schedule is read against the host's own zone — so the instants are
        // the ones whose *wall clock* is that Monday, wherever this runs.
        let wall_to_instant =
            |wall: u64| wall.saturating_add_signed(cron_zone().offset_secs.saturating_neg());
        let now = wall_to_instant(1_704_099_900); // local 2024-01-01 09:05, Monday.
        let due_at = wall_to_instant(1_704_099_600); // local 2024-01-01 09:00, Monday.
        let root = std::env::temp_dir().join(format!(
            "cron-run-due-tool-test-{}-{}",
            now,
            std::process::id()
        ));
        let path = root.join("crons.json");
        std::fs::create_dir_all(&root).expect("create temp root");
        std::fs::write(
            &path,
            json!({
                "counter": 3,
                "entries": {
                    "cron_due": {
                        "cron_id": "cron_due",
                        "schedule": "0 9 * JAN MON",
                        "prompt": "due prompt",
                        "description": "due description",
                        "enabled": true,
                        "created_at": now - 3600,
                        "updated_at": now - 3600,
                        "last_run_at": null,
                        "run_count": 0
                    },
                    "cron_not_due": {
                        "cron_id": "cron_not_due",
                        "schedule": "0 10 * JAN MON",
                        "prompt": "not due prompt",
                        "description": null,
                        "enabled": true,
                        "created_at": now - 3600,
                        "updated_at": now - 3600,
                        "last_run_at": null,
                        "run_count": 0
                    },
                    "cron_disabled": {
                        "cron_id": "cron_disabled",
                        "schedule": "0 9 * JAN MON",
                        "prompt": "disabled prompt",
                        "description": null,
                        "enabled": false,
                        "created_at": now - 3600,
                        "updated_at": now - 3600,
                        "last_run_at": null,
                        "run_count": 0
                    }
                }
            })
            .to_string(),
        )
        .expect("write cron registry");

        let crons = CronRegistry::with_persistence_path(Some(path.clone()));
        let tasks = TaskRegistry::new_in_memory();
        let preview = parse_json(
            &run_cron_run_due_at(&crons, &tasks, &CronRunDueInput { dry_run: true }, now)
                .expect("dry run should succeed"),
        );
        assert!(preview["dry_run"].as_bool().unwrap());
        assert_eq!(preview["due_count"], 1);
        assert_eq!(preview["created_task_ids"].as_array().unwrap().len(), 0);
        assert_eq!(tasks.len(), 0);
        assert_eq!(crons.get("cron_due").unwrap().run_count, 0);

        let output = parse_json(
            &run_cron_run_due_at(&crons, &tasks, &CronRunDueInput { dry_run: false }, now)
                .expect("run due should succeed"),
        );
        assert_eq!(output["scheduler_status"], "manual_run_due");
        assert_eq!(output["automatic_scheduler_status"], "session_idle");
        assert_eq!(output["due_count"], 1);
        let task_id = output["created_task_ids"].as_array().unwrap()[0]
            .as_str()
            .unwrap();
        let task = tasks.get(task_id).expect("created task");
        assert_eq!(task.prompt, "due prompt");
        assert_eq!(task.status, TaskStatus::Created);
        let due_cron = crons.get("cron_due").expect("due cron");
        assert_eq!(due_cron.run_count, 1);
        assert_eq!(due_cron.last_run_at, Some(due_at));

        let duplicate = parse_json(
            &run_cron_run_due_at(&crons, &tasks, &CronRunDueInput { dry_run: false }, now)
                .expect("duplicate due run should succeed"),
        );
        assert_eq!(duplicate["due_count"], 0);
        assert_eq!(tasks.len(), 1);
        assert_eq!(crons.get("cron_due").unwrap().run_count, 1);

        std::fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn team_create_output_says_execution_is_not_wired() {
        let teams = TeamRegistry::new_in_memory();
        let tasks = TaskRegistry::new_in_memory();

        let output = run_team_create(
            &teams,
            &tasks,
            TeamCreateInput {
                name: "records".to_string(),
                tasks: vec![TeamTaskInput::InlineTask {
                    prompt: "tracked only".to_string(),
                    description: None,
                }],
            },
        )
        .expect("team create should succeed");

        let json = parse_json(&output);
        assert_eq!(json["execution_status"], "not_wired");
        assert!(json["message"].as_str().unwrap().contains("not wired"));
        let task_id = json["task_ids"].as_array().unwrap()[0].as_str().unwrap();
        assert_eq!(
            tasks.get(task_id).expect("inline task").status,
            TaskStatus::Created,
            "TeamCreate should remain record-only and must not start execution"
        );
    }
}
