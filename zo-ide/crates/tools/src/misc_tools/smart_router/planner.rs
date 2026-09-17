use runtime::{
    RouteConfidence, RouteDiversityNeed, RouteRole, RouteTaskKind, RouteTaskRisk,
    RouteToolNeed, RouteVerificationNeed,
};

use super::metadata::TaskRouteMetadata;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentNeedPlan {
    pub candidate_role: RouteRole,
    pub need: String,
    pub evidence_target: String,
    pub stop_condition: String,
    pub fallback: String,
    pub confidence: RouteConfidence,
}

impl AgentNeedPlan {
    fn new(
        candidate_role: RouteRole,
        need: &str,
        evidence_target: &str,
        stop_condition: &str,
        fallback: &str,
        confidence: RouteConfidence,
    ) -> Self {
        Self {
            candidate_role,
            need: need.to_string(),
            evidence_target: evidence_target.to_string(),
            stop_condition: stop_condition.to_string(),
            fallback: fallback.to_string(),
            confidence,
        }
    }
}

pub(super) fn plan_agent_needs(metadata: &TaskRouteMetadata) -> Vec<AgentNeedPlan> {
    let mut plans = Vec::new();

    if matches!(metadata.verification_need, RouteVerificationNeed::Focused | RouteVerificationNeed::Full) {
        plans.push(AgentNeedPlan::new(
            if matches!(metadata.kind, RouteTaskKind::Review) { RouteRole::Reviewer } else { RouteRole::Verifier },
            "verify the task-specific acceptance evidence",
            match metadata.verification_need {
                RouteVerificationNeed::Full => "full requested verification evidence",
                _ => "focused verification evidence",
            },
            "verdict is pass/fail/unknown with cited evidence",
            "escalate to shape selector as unknown verification ownership",
            metadata.confidence,
        ));
    }

    if matches!(metadata.kind, RouteTaskKind::Debugging) {
        plans.push(AgentNeedPlan::new(
            RouteRole::Debugging,
            "reduce reproduction/root-cause uncertainty",
            "reproduction steps or failing check evidence",
            "root cause identified or reproduction marked blocked",
            "return to sequential workflow with blocked evidence",
            metadata.confidence,
        ));
    }

    if matches!(metadata.risk, RouteTaskRisk::High | RouteTaskRisk::Critical) && !has_role(&plans, RouteRole::Reviewer) {
        plans.push(AgentNeedPlan::new(
            RouteRole::Reviewer,
            "independently review high-risk behavior",
            "risk-specific review findings with affected symbols or paths",
            "review verdict includes concrete pass/fail evidence",
            "rerun only if new risk evidence invalidates the review",
            RouteConfidence::Medium,
        ));
    }

    if matches!(metadata.diversity_need, RouteDiversityNeed::Required) && !has_role(&plans, RouteRole::Judge) {
        plans.push(AgentNeedPlan::new(
            RouteRole::Judge,
            "provide independent adjudication for conflicting evidence",
            "structured comparison of candidate outcomes",
            "judge selects or declares an explicit tie",
            "ask for clarification when evidence remains tied",
            metadata.confidence,
        ));
    }

    if matches!(metadata.kind, RouteTaskKind::Coding) && matches!(metadata.tool_need, RouteToolNeed::Write) && matches!(metadata.risk, RouteTaskRisk::Medium | RouteTaskRisk::High | RouteTaskRisk::Critical) {
        plans.push(AgentNeedPlan::new(
            RouteRole::Coding,
            "implement a bounded risky code slice",
            "patch plus local verification evidence",
            "patch is applied or blocked with reason",
            "fall back to main-agent sequential implementation",
            metadata.confidence,
        ));
    }

    plans
}

fn has_role(plans: &[AgentNeedPlan], role: RouteRole) -> bool {
    plans.iter().any(|plan| plan.candidate_role == role)
}
