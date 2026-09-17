//! `ToolGateway` phase-1 shim.
//!
//! This is deliberately a metadata envelope around the existing dispatcher, not
//! a new execution engine. It records a normalized request, the structured
//! permission/policy pre-check, and standardized result metadata while leaving
//! every concrete tool handler and its user-visible output unchanged.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use runtime::permission_enforcer::{EnforcementResult, PermissionEnforcer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifacts::ArtifactRef;
use crate::error::ToolError;

const TOOL_INVOCATION_SCHEMA_VERSION: u32 = 1;

static TOOL_INVOCATION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub schema_version: u32,
    pub invocation_id: String,
    pub request: ToolInvocationRequest,
    pub policy_decision: ToolPolicyDecision,
    pub result: ToolInvocationResult,
    pub started_at_epoch_ms: u128,
    pub completed_at_epoch_ms: u128,
    pub duration_ms: u128,
    /// Workflow run this call belongs to, when one was active (`record_tool_invocation`
    /// stamps it from the context). Lets the audit join tool calls to a workflow
    /// run's `WorkflowEventRecord` stream by `run_id` (WI-C).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Foreground turn this call belongs to, when one was active. Joins tool
    /// calls to the turn that issued them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocationRequest {
    pub requested_name: String,
    pub tool_name: String,
    pub family: ToolFamily,
    pub input_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFamily {
    Bash,
    File,
    Web,
    Task,
    Worker,
    Team,
    Cron,
    Mcp,
    Misc,
    Worktree,
    PlanMode,
    Workflow,
    Plugin,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ToolPolicyDecision {
    NotConfigured,
    Allowed {
        active_mode: Option<String>,
        required_mode: Option<String>,
    },
    Denied {
        check: ToolPolicyCheck,
        active_mode: String,
        required_mode: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyCheck {
    ToolPermission,
    BashCommandIntent,
    ToolToggle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolInvocationResult {
    Succeeded {
        metadata: ToolResultMetadata,
    },
    Failed {
        error_kind: ToolErrorKind,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultMetadata {
    /// Character count after tool-specific enrichments but before the global
    /// truncation middleware.
    pub output_chars: usize,
    /// Character count returned to the caller/model after truncation.
    pub returned_chars: usize,
    pub truncated: bool,
    /// Phase-4: when the output was truncated for the transcript/model, the full
    /// pre-truncation content is preserved content-addressed in the artifact
    /// store and tracked here, so it stays recoverable without bloating context.
    /// `None` when the output fit untruncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorKind {
    NotFound,
    PermissionDenied,
    InvalidInput,
    Execution,
    Declined,
    Io,
    Json,
    PluginConflict,
    DuplicateName,
}

// ---------------------------------------------------------------------------
// Audit surface (WI-E2)
// ---------------------------------------------------------------------------
//
// The ledger above is populated on every dispatch but had no production
// consumer. `AuditSummary` is a pure rollup of the recorded invocations — the
// "what ran, what was allowed/denied, what failed" view a user / CI / the model
// needs — surfaced through `ToolContext::audit_summary` and the `Audit` tool.

/// Aggregated, serializable view of the [`ToolInvocation`] ledger.
// `Eq` is intentionally not derived: `RouteDecisionRecord::confidence` is an
// `f32`, which is `PartialEq` but not `Eq`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuditSummary {
    /// Total recorded invocations.
    pub total: usize,
    /// Invocations the policy allowed.
    pub allowed: usize,
    /// Invocations the policy denied (also enumerated in `denials`).
    pub denied: usize,
    /// Invocations dispatched without an enforcer (`NotConfigured`).
    pub not_configured: usize,
    /// Invocations whose handler returned a result.
    pub succeeded: usize,
    /// Invocations whose handler returned an error.
    pub failed: usize,
    /// Invocation count per tool family (stable order, omitted when empty).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub by_family: BTreeMap<ToolFamily, usize>,
    /// Every denied invocation with its policy check and reason.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub denials: Vec<AuditDenial>,
    /// Content-addressed artifacts produced by truncated outputs, kept linkable
    /// from the audit so the full pre-truncation content stays recoverable.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRef>,
    /// Structured route decisions recorded this session (WI-C). Folded in by
    /// `ToolContext::audit_summary`; `summarize_invocations` leaves it empty since
    /// the ledger lives beside the invocation ledger, not inside it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub route_decisions: Vec<RouteDecisionRecord>,
}

/// One denied invocation in an [`AuditSummary`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditDenial {
    pub invocation_id: String,
    pub tool_name: String,
    pub check: ToolPolicyCheck,
    pub active_mode: String,
    pub required_mode: String,
    pub reason: String,
}

/// One structured route decision recorded by the turn controller (WI-C),
/// replacing the prior `ZO_ROUTE_DEBUG` eprintln. `shape` is the advisory
/// route shape (`solo` / `delegate_one` / `pipeline` / …); `host_prespawn` is
/// whether the host actually engaged the pre-spawn path, while
/// `semantic_triage` tracks the cheap intent-routing prelude separately and
/// `intent` exposes the resolved deliverable axis used by design behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteDecisionRecord {
    pub shape: String,
    /// The host `shape` expressed in the model router's canonical taxonomy
    /// (`runtime::RouteShapeKind` label), so the host pre-spawn decision and the
    /// per-agent model-routing decision are recorded in one shared vocabulary.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub canonical_shape: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub intent: String,
    pub confidence: f32,
    pub host_prespawn: bool,
    #[serde(default)]
    pub semantic_triage: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

/// Roll the recorded invocation ledger up into an [`AuditSummary`]. Pure over
/// the slice — no new tracking, just a read of what `record_tool_invocation`
/// already captured.
#[must_use]
pub fn summarize_invocations(invocations: &[ToolInvocation]) -> AuditSummary {
    let mut summary = AuditSummary {
        total: invocations.len(),
        ..AuditSummary::default()
    };
    for invocation in invocations {
        *summary
            .by_family
            .entry(invocation.request.family)
            .or_default() += 1;
        match &invocation.policy_decision {
            ToolPolicyDecision::Allowed { .. } => summary.allowed += 1,
            ToolPolicyDecision::NotConfigured => summary.not_configured += 1,
            ToolPolicyDecision::Denied {
                check,
                active_mode,
                required_mode,
                reason,
            } => {
                summary.denied += 1;
                summary.denials.push(AuditDenial {
                    invocation_id: invocation.invocation_id.clone(),
                    tool_name: invocation.request.tool_name.clone(),
                    check: *check,
                    active_mode: active_mode.clone(),
                    required_mode: required_mode.clone(),
                    reason: reason.clone(),
                });
            }
        }
        match &invocation.result {
            ToolInvocationResult::Succeeded { metadata } => {
                summary.succeeded += 1;
                if let Some(artifact) = &metadata.artifact {
                    summary.artifacts.push(artifact.clone());
                }
            }
            ToolInvocationResult::Failed { .. } => summary.failed += 1,
        }
    }
    summary
}

#[derive(Debug, Clone)]
pub(crate) struct ToolInvocationStart {
    invocation_id: String,
    request: ToolInvocationRequest,
    policy_decision: ToolPolicyDecision,
    started_at_epoch_ms: u128,
    started: Instant,
}

impl ToolInvocationStart {
    #[must_use]
    pub(crate) fn with_family(mut self, family: ToolFamily) -> Self {
        self.request.family = family;
        self
    }

    #[must_use]
    pub(crate) fn with_policy_decision(mut self, policy_decision: ToolPolicyDecision) -> Self {
        self.policy_decision = policy_decision;
        self
    }

    #[must_use]
    pub(crate) fn finish(
        self,
        result: ToolInvocationResult,
        completed_at_epoch_ms: u128,
    ) -> ToolInvocation {
        ToolInvocation {
            schema_version: TOOL_INVOCATION_SCHEMA_VERSION,
            invocation_id: self.invocation_id,
            request: self.request,
            policy_decision: self.policy_decision,
            result,
            started_at_epoch_ms: self.started_at_epoch_ms,
            completed_at_epoch_ms,
            duration_ms: self.started.elapsed().as_millis(),
            // Stamped by `ToolContext::record_tool_invocation` from the active
            // run/turn at record time, so this construction stays context-free.
            run_id: None,
            turn_id: None,
        }
    }
}

#[must_use]
pub(crate) fn begin_tool_invocation(
    requested_name: &str,
    tool_name: &str,
    input: &Value,
    enforcer: Option<&PermissionEnforcer>,
) -> ToolInvocationStart {
    let sequence = TOOL_INVOCATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    ToolInvocationStart {
        invocation_id: format!("tool-invocation-{sequence}"),
        request: ToolInvocationRequest {
            requested_name: requested_name.to_owned(),
            tool_name: tool_name.to_owned(),
            family: ToolFamily::for_tool(tool_name),
            input_bytes: input.to_string().len(),
        },
        policy_decision: classify_policy_decision(enforcer, tool_name, input),
        started_at_epoch_ms: epoch_millis_now(),
        started: Instant::now(),
    }
}

#[must_use]
pub(crate) fn successful_result(metadata: ToolResultMetadata) -> ToolInvocationResult {
    ToolInvocationResult::Succeeded { metadata }
}

#[must_use]
pub(crate) fn failed_result(error: &ToolError) -> ToolInvocationResult {
    ToolInvocationResult::Failed {
        error_kind: ToolErrorKind::from_error(error),
        message: error.to_string(),
    }
}

#[must_use]
pub(crate) fn toggle_denied_decision(reason: &str) -> ToolPolicyDecision {
    ToolPolicyDecision::Denied {
        check: ToolPolicyCheck::ToolToggle,
        active_mode: "tool-toggle".to_owned(),
        required_mode: "enabled".to_owned(),
        reason: reason.to_owned(),
    }
}

#[must_use]
pub(crate) fn epoch_millis_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

impl ToolFamily {
    #[must_use]
    pub(crate) fn for_tool(tool_name: &str) -> Self {
        match tool_name {
            "bash" | "REPL" | "PowerShell" => Self::Bash,
            "read_file" | "read_image" | "write_file" | "edit_file" | "InstrumentLog"
            | "DebugHypothesis" | "glob_search" | "grep_search" => Self::File,
            "WebFetch" | "WebSearch" => Self::Web,
            "TodoWrite" | "TaskGet" | "TaskList" | "TaskStop" | "TaskOutput" | "TaskUpdate" => {
                Self::Task
            }
            "WorkerCreate" | "WorkerGet" | "WorkerObserve" | "WorkerResolveTrust"
            | "WorkerAwaitReady" | "WorkerSendPrompt" | "WorkerRestart" | "WorkerTerminate" => {
                Self::Worker
            }
            "TeamCreate" | "TeamDelete" | "TeamInboxPost" | "TeamInboxJoin"
            | "TeamInboxChannels" | "TeamInboxUnread" | "TeamInboxAck" | "TeamInboxLeave" => {
                Self::Team
            }
            "CronCreate" | "CronDelete" | "CronList" | "CronRunDue" => Self::Cron,
            "LSP" | "ListMcpResources" | "ReadMcpResource" | "McpAuth" | "MCP" => Self::Mcp,
            "EnterWorktree" | "ExitWorktree" => Self::Worktree,
            "EnterPlanMode" | "ExitPlanMode" | "ExitPlanModeV2" => Self::PlanMode,
            "Workflow" | "WorkflowValidate" | "WorkflowLibrary" | "WorkflowRuns"
            | "WorkflowSkillProject" => Self::Workflow,
            "Skill" | "SkillDistill" | "SkillReview" | "ToolSearch" | "NotebookEdit" | "Sleep"
            | "SendUserMessage" | "Brief" | "SyntheticOutput" | "SpawnMultiAgent" | "Council"
            | "Config" | "StructuredOutput" | "AskUserQuestion" | "MemoryWrite"
            | "RemoteTrigger" | "Monitor" | "SendMessage" | "ListAgents" | "PushNotification"
            | "ScheduleWakeup" | "session_recall" | "Audit" | "StopAgent" => Self::Misc,
            _ if tool_name.starts_with("mcp__") => Self::Mcp,
            _ => Self::Unknown,
        }
    }
}

impl ToolErrorKind {
    #[must_use]
    pub(crate) const fn from_error(error: &ToolError) -> Self {
        match error {
            ToolError::NotFound(_) => Self::NotFound,
            ToolError::PermissionDenied { .. } => Self::PermissionDenied,
            ToolError::InvalidInput(_) => Self::InvalidInput,
            ToolError::Execution(_) => Self::Execution,
            ToolError::Declined(_) => Self::Declined,
            ToolError::Io(_) => Self::Io,
            ToolError::Json(_) => Self::Json,
            ToolError::PluginConflict(_) => Self::PluginConflict,
            ToolError::DuplicateName(_) => Self::DuplicateName,
        }
    }
}

fn classify_policy_decision(
    enforcer: Option<&PermissionEnforcer>,
    tool_name: &str,
    input: &Value,
) -> ToolPolicyDecision {
    let Some(enforcer) = enforcer else {
        return ToolPolicyDecision::NotConfigured;
    };

    let input_str = serde_json::to_string(input).unwrap_or_default();
    if let EnforcementResult::Denied {
        active_mode,
        required_mode,
        reason,
        ..
    } = enforcer.check(tool_name, &input_str)
    {
        return ToolPolicyDecision::Denied {
            check: ToolPolicyCheck::ToolPermission,
            reason,
            active_mode,
            required_mode,
        };
    }

    if tool_name == "bash" {
        if let Some(command) = input.get("command").and_then(Value::as_str) {
            if let EnforcementResult::Denied {
                active_mode,
                required_mode,
                reason,
                ..
            } = enforcer.check_bash(command)
            {
                return ToolPolicyDecision::Denied {
                    check: ToolPolicyCheck::BashCommandIntent,
                    reason,
                    active_mode,
                    required_mode,
                };
            }
        }
    }

    ToolPolicyDecision::Allowed {
        active_mode: Some(enforcer.active_mode().as_str().to_owned()),
        required_mode: required_mode_for_builtin(tool_name),
    }
}

fn required_mode_for_builtin(tool_name: &str) -> Option<String> {
    crate::registry::mvp_tool_specs()
        .iter()
        .find(|spec| spec.name == tool_name)
        .map(|spec| spec.required_permission.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok_result() -> ToolInvocationResult {
        successful_result(ToolResultMetadata {
            output_chars: 10,
            returned_chars: 10,
            truncated: false,
            artifact: None,
        })
    }

    fn invocation(
        name: &str,
        decision: ToolPolicyDecision,
        result: ToolInvocationResult,
    ) -> ToolInvocation {
        begin_tool_invocation(name, name, &json!({}), None)
            .with_policy_decision(decision)
            .finish(result, epoch_millis_now())
    }

    fn allowed() -> ToolPolicyDecision {
        ToolPolicyDecision::Allowed {
            active_mode: Some("workspace-write".to_owned()),
            required_mode: None,
        }
    }

    #[test]
    fn summarize_rolls_up_outcomes_denials_and_families() {
        let invocations = vec![
            invocation("bash", allowed(), ok_result()),
            invocation("read_file", ToolPolicyDecision::NotConfigured, ok_result()),
            invocation(
                "bash",
                toggle_denied_decision("tool disabled"),
                failed_result(&ToolError::PermissionDenied {
                    tool: "bash".to_owned(),
                    reason: "denied".to_owned(),
                }),
            ),
            invocation(
                "write_file",
                allowed(),
                failed_result(&ToolError::Execution("boom".to_owned())),
            ),
        ];
        let summary = summarize_invocations(&invocations);

        assert_eq!(summary.total, 4);
        assert_eq!(summary.allowed, 2);
        assert_eq!(summary.denied, 1);
        assert_eq!(summary.not_configured, 1);
        assert_eq!(summary.succeeded, 2);
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.denials.len(), 1);
        assert_eq!(summary.denials[0].tool_name, "bash");
        assert_eq!(summary.denials[0].check, ToolPolicyCheck::ToolToggle);
        // read_file + write_file both map to the File family.
        assert_eq!(summary.by_family.get(&ToolFamily::Bash).copied(), Some(2));
        assert_eq!(summary.by_family.get(&ToolFamily::File).copied(), Some(2));
    }

    #[test]
    fn empty_ledger_summarizes_to_zeroes() {
        let summary = summarize_invocations(&[]);
        assert_eq!(summary, AuditSummary::default());
        assert_eq!(summary.total, 0);
    }

    #[test]
    fn summary_serializes_family_map_with_string_keys() {
        // `BTreeMap<ToolFamily, usize>` must serialize as a JSON object keyed by
        // the snake_case family name, not error on a non-string map key.
        let summary = summarize_invocations(&[invocation(
            "bash",
            ToolPolicyDecision::NotConfigured,
            ok_result(),
        )]);
        let value = serde_json::to_value(&summary).expect("AuditSummary serializes");
        assert_eq!(value["by_family"]["bash"], 1);
        assert_eq!(value["total"], 1);
        // Empty collections are omitted.
        assert!(value.get("denials").is_none());
        assert!(value.get("artifacts").is_none());
    }
}
