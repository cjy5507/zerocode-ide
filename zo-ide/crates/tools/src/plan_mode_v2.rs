//! `ExitPlanModeV2` — the plan-submission half of the plan-approval gate. The
//! model, while the session is read-only (plan mode), submits a structured plan
//! here. The plan is persisted as an editable `.zo/plans/<id>.md` artifact
//! and returned, but **write permission is never restored by this tool** — that
//! would let the model self-approve its own plan. A human approves it
//! out-of-band (the TUI `/plan off`), which is what restores write access.
//!
//! It is therefore a `ReadOnly` tool (callable from within plan mode) whose
//! only side effect is writing plan metadata under `.zo/`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{to_pretty_json, ToolError, ToolSpec};
use runtime::PermissionMode;

#[derive(Debug, Deserialize)]
pub(crate) struct ExitPlanModeV2Input {
    pub plan: String,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExitPlanModeV2Output {
    status: &'static str,
    plan: String,
    summary: Option<String>,
    /// Always `false`: submitting a plan never exits plan mode / restores write.
    /// Approval is a human action (`/plan off`), never a model action.
    #[serde(rename = "planModeExited")]
    plan_mode_exited: bool,
    #[serde(rename = "planPath", skip_serializing_if = "Option::is_none")]
    plan_path: Option<String>,
    message: String,
}

#[must_use]
pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "ExitPlanModeV2",
        description: "Submit and persist a plan for user approval. Never grants write access; wait for approval.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "plan": { "type": "string" },
                "summary": { "type": "string" }
            },
            "required": ["plan"],
            "additionalProperties": false
        }),
        // ReadOnly so the model can submit a plan from within plan mode (a
        // read-only session); the tool grants no write access of its own.
        required_permission: PermissionMode::ReadOnly,
    }]
}

pub(crate) fn run_exit_plan_mode_v2(input: ExitPlanModeV2Input) -> Result<String, ToolError> {
    if input.plan.trim().is_empty() {
        return Err(ToolError::InvalidInput(
            "plan must not be empty".to_string(),
        ));
    }

    let artifact = crate::misc_tools::write_plan_artifact(&input.plan, input.summary.as_deref());
    to_pretty_json(build_v2_output(input.plan, input.summary, artifact))
}

/// Build the tool output from the plan and the (best-effort) artifact-write
/// result. Pure — no IO — so the gate's defining property is unit-testable:
/// `plan_mode_exited` is always `false`, so submitting a plan can never restore
/// write permission. A failed artifact write degrades gracefully (no path, an
/// honest message) rather than failing the submission.
fn build_v2_output(
    plan: String,
    summary: Option<String>,
    artifact: Result<PathBuf, String>,
) -> ExitPlanModeV2Output {
    let (plan_path, message) = match artifact {
        Ok(path) => {
            let shown = path.display().to_string();
            let message = format!(
                "Plan recorded at {shown}. It has NOT been approved — the user must approve it (e.g. `/plan off`) before you make any edits."
            );
            (Some(shown), message)
        }
        Err(error) => (
            None,
            format!(
                "Plan submitted but could not be saved to disk: {error}. It has NOT been approved — wait for the user to approve before editing."
            ),
        ),
    };
    ExitPlanModeV2Output {
        status: "ok",
        plan,
        summary,
        plan_mode_exited: false,
        plan_path,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_plan_mode_v2_spec_is_read_only() {
        let specs = tool_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "ExitPlanModeV2");
        // ReadOnly so it is callable from within plan mode and grants no write.
        assert_eq!(specs[0].required_permission, PermissionMode::ReadOnly);
    }

    #[test]
    fn build_v2_output_never_exits_plan_mode() {
        // The gate's defining property: submitting a plan does not restore
        // write permission, and the message tells the model to wait for approval.
        let out = build_v2_output(
            "step 1: do the thing".to_string(),
            Some("one step plan".to_string()),
            Ok(PathBuf::from("/repo/.zo/plans/plan-abc.md")),
        );
        assert!(!out.plan_mode_exited, "must never self-exit / self-approve");
        assert_eq!(out.status, "ok");
        assert_eq!(out.plan, "step 1: do the thing");
        assert_eq!(
            out.plan_path.as_deref(),
            Some("/repo/.zo/plans/plan-abc.md")
        );
        assert!(out.message.contains("approve"), "got {}", out.message);
    }

    #[test]
    fn build_v2_output_degrades_when_artifact_write_fails() {
        let out = build_v2_output("step 1".to_string(), None, Err("disk full".to_string()));
        assert!(!out.plan_mode_exited);
        assert!(out.plan_path.is_none());
        assert!(
            out.message.contains("could not be saved"),
            "got {}",
            out.message
        );
        assert!(out.message.contains("approve"));
    }

    #[test]
    fn exit_plan_mode_v2_rejects_empty_plan() {
        // Empty plan errors *before* any artifact is written (no IO / no cwd).
        let err = run_exit_plan_mode_v2(ExitPlanModeV2Input {
            plan: "   ".to_string(),
            summary: None,
        })
        .expect_err("should reject empty plan");
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
