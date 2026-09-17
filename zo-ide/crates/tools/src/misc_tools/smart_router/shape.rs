use runtime::{RouteConfidence, RouteShapeKind, RouteTaskComplexity, RouteTaskRisk};

use super::metadata::TaskRouteMetadata;
use super::planner::AgentNeedPlan;

pub(super) type RouteShape = RouteShapeKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UserOrchestrationRequestOutcome {
    AcceptRequestedShape,
    RecommendDifferentShape,
    NeedClarification,
    RefuseUnsafeShape,
    NotRequested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RouteShapeDecision {
    pub shape: RouteShape,
    pub outcome: UserOrchestrationRequestOutcome,
    pub reason: String,
    pub confidence: RouteConfidence,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RouteShapeInput<'a> {
    pub metadata: &'a TaskRouteMetadata,
    pub needs: &'a [AgentNeedPlan],
    pub requested_shape: Option<RouteShape>,
    pub independent_lanes: usize,
    pub has_findings: bool,
    pub ambiguous_ownership: bool,
    pub unsafe_request: bool,
}

impl<'a> RouteShapeInput<'a> {
    pub(super) fn new(metadata: &'a TaskRouteMetadata, needs: &'a [AgentNeedPlan]) -> Self {
        Self {
            metadata,
            needs,
            requested_shape: None,
            independent_lanes: 0,
            has_findings: false,
            ambiguous_ownership: false,
            unsafe_request: false,
        }
    }

    #[cfg(test)]
    pub(super) fn with_requested_shape(mut self, requested_shape: RouteShape) -> Self {
        self.requested_shape = Some(requested_shape);
        self
    }

    #[cfg(test)]
    pub(super) fn with_independent_lanes(mut self, independent_lanes: usize) -> Self {
        self.independent_lanes = independent_lanes;
        self
    }

    #[cfg(test)]
    pub(super) fn with_findings(mut self, has_findings: bool) -> Self {
        self.has_findings = has_findings;
        self
    }

    #[cfg(test)]
    pub(super) fn with_ambiguous_ownership(mut self, ambiguous_ownership: bool) -> Self {
        self.ambiguous_ownership = ambiguous_ownership;
        self
    }

    #[cfg(test)]
    pub(super) fn with_unsafe_request(mut self, unsafe_request: bool) -> Self {
        self.unsafe_request = unsafe_request;
        self
    }
}

pub(super) fn select_route_shape(input: &RouteShapeInput<'_>) -> RouteShapeDecision {
    if input.unsafe_request {
        return RouteShapeDecision {
            shape: RouteShape::SequentialWorkflow,
            outcome: UserOrchestrationRequestOutcome::RefuseUnsafeShape,
            reason: "requested orchestration is unsafe; use a controlled sequential workflow".to_string(),
            confidence: RouteConfidence::High,
        };
    }

    if input.ambiguous_ownership {
        return RouteShapeDecision {
            shape: RouteShape::SequentialWorkflow,
            outcome: input
                .requested_shape
                .map_or(UserOrchestrationRequestOutcome::NeedClarification, |_| {
                    UserOrchestrationRequestOutcome::RecommendDifferentShape
                }),
            reason: "lane ownership is ambiguous; classify or serialize before spawning implementers".to_string(),
            confidence: RouteConfidence::Medium,
        };
    }

    let natural = natural_shape(input);
    let outcome = match input.requested_shape {
        None => UserOrchestrationRequestOutcome::NotRequested,
        Some(requested) if requested == natural => UserOrchestrationRequestOutcome::AcceptRequestedShape,
        Some(_) if safe_to_honor_requested(input) => UserOrchestrationRequestOutcome::RecommendDifferentShape,
        Some(_) => UserOrchestrationRequestOutcome::NeedClarification,
    };

    RouteShapeDecision {
        shape: natural,
        outcome,
        reason: reason_for(natural).to_string(),
        confidence: shape_confidence(input, natural),
    }
}

fn natural_shape(input: &RouteShapeInput<'_>) -> RouteShape {
    if input.has_findings && input.independent_lanes >= 2 {
        RouteShape::ParallelRepairLoop
    } else if input.has_findings {
        RouteShape::RepairLoop
    } else if input.independent_lanes >= 2 {
        // Checked BEFORE `needs.is_empty()`: a fan-out member carries its lane count
        // from the fanout position, so a parallel spawn must route per-role even when
        // no agent-need plan was synthesized. Otherwise every lane falls through to
        // Solo and silently inherits the parent model.
        RouteShape::ParallelLanes
    } else if input.needs.is_empty() {
        RouteShape::Solo
    } else if input.needs.len() == 1 {
        RouteShape::OneSpecialist
    } else if matches!(input.metadata.complexity, RouteTaskComplexity::Large) || matches!(input.metadata.risk, RouteTaskRisk::High | RouteTaskRisk::Critical) {
        RouteShape::SequentialWorkflow
    } else {
        RouteShape::OneSpecialist
    }
}

fn safe_to_honor_requested(input: &RouteShapeInput<'_>) -> bool {
    !matches!(input.metadata.risk, RouteTaskRisk::Critical) && !input.ambiguous_ownership
}

fn shape_confidence(input: &RouteShapeInput<'_>, shape: RouteShape) -> RouteConfidence {
    match shape {
        RouteShape::Solo if input.needs.is_empty() => RouteConfidence::High,
        RouteShape::ParallelLanes | RouteShape::ParallelRepairLoop if input.independent_lanes >= 2 => RouteConfidence::High,
        RouteShape::RepairLoop if input.has_findings => RouteConfidence::High,
        _ => input.metadata.confidence,
    }
}

fn reason_for(shape: RouteShape) -> &'static str {
    match shape {
        RouteShape::Solo => "no agent need plan adds unique evidence",
        RouteShape::OneSpecialist => "one bounded uncertainty has a specific evidence target",
        RouteShape::SequentialWorkflow => "work should be serialized because ownership or risk is coupled",
        RouteShape::ParallelLanes => "independent lanes have separable ownership and verification",
        RouteShape::RepairLoop => "concrete findings need fixer plus focused reverify",
        RouteShape::ParallelRepairLoop => "independent findings or lanes can be repaired and verified separately",
    }
}
