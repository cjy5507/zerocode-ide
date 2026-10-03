use std::path::Path;
use std::sync::Arc;

use runtime::{DecisionKind, PlanCandidate, PlanContext, PlanShape, VerdictBasis, VerifyMode};

use super::{BuiltRuntime, smart_runtime::PlanShadowTurn, turn_harness::TurnSetup};

#[derive(Clone, Copy)]
pub(crate) enum ModelSelection {
    Automatic,
    Pinned,
}

pub(crate) struct PlanTurn {
    receipt: tools::PlanRunReceipt,
    began_ms: u64,
    pub(crate) selected: Option<PlanCandidate>,
}

pub(crate) struct PlanStart<'a> {
    pub(crate) shadow: &'a PlanShadowTurn,
    pub(crate) setup: &'a TurnSetup,
    pub(crate) input: &'a str,
    pub(crate) session: &'a str,
    pub(crate) began_ms: u64,
    pub(crate) pinned_model: bool,
    pub(crate) prelude: tools::HostPrelude,
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .ok().and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok()).unwrap_or(0)
}

impl PlanTurn {
    pub(crate) fn begin(runtime: &mut BuiltRuntime, start: &PlanStart<'_>) -> Option<Self> {
        let cohort = tools::plan_cohort_for_turn(start.input, start.setup.assessment.complexity, start.setup.orchestration.risk)?;
        let current = api::resolve_model_alias(runtime.api_client().model());
        let shape = super::smart_runtime::plan_shape_of(start.prelude);
        let objective = runtime.deep_gate().is_some_and(|gate| gate.check_command.is_some());
        let actual = tools::PlanShadowActual {
            model: current.clone(), effort: start.shadow.effort.clone(), shape: shape.label(),
            verify: if objective { VerifyMode::Objective } else { VerifyMode::None }.as_str().into(),
        };
        let attempt = runtime.try_runtime()?.next_attempt();
        let context = runtime.try_runtime_mut().map_or(0, |inner| u64::try_from(inner.estimated_tokens()).unwrap_or(u64::MAX));
        let priors = tools::plan_priors_for(start.setup.assessment.complexity)?;
        let ctx = PlanContext {
            cohort: Some(&cohort), complexity: start.setup.assessment.complexity,
            current_model: &current, current_effort: start.shadow.effort.as_deref(), context_tokens: context,
            pinned_model: start.setup.orchestration.user_named_model.or(start.pinned_model.then_some(current.as_str())),
            pinned_effort: start.shadow.pinned_effort.as_deref(),
            attended: matches!(runtime::declared_attendance(), runtime::Attendance::Attended),
            objective_check_available: objective, independent_slices: None, max_lanes: 1,
            verify_ceiling: 1, min_p_verified: f64::from(start.shadow.settings.min_pass_percent.max(90)) / 100.0,
            switch_margin: f64::from(start.shadow.settings.switch_margin_percent.max(10)) / 100.0,
        };
        let inputs = tools::PlanShadowInputs {
            session_id: start.session, attempt: Some(&attempt), recorded_at: now_ms() / 1_000,
            ctx, models: &start.shadow.models, priors: &priors, records: &[],
            price: &tools::model_price_for, cache: &[], actual: actual.clone(), trigger: None, category: None,
        };
        let selected = (start.shadow.settings.apply && start.shadow.held.is_none()
            && start.shadow.effort.is_some() && shape == PlanShape::Solo
            && !start.setup.orchestration.user_requested_delegation())
            .then(|| tools::read_plan_receipts(&start.shadow.cwd).ok()
                .and_then(|receipts| tools::choose_measured_plan(&inputs, &receipts)))
            .flatten().filter(|plan| tools::plan_pins_allow(&inputs.ctx, plan));
        Some(Self {
            receipt: tools::PlanRunReceipt {
                version: 1, at_ms: 0, attempt, cohort, plan: actual, verified: None,
                duration_ms: 0, total_tokens: None, total_usd: None,
                held: start.shadow.held.into_iter().map(str::to_string).collect(),
            },
            began_ms: start.began_ms, selected,
        })
    }

    pub(crate) fn client(
        &mut self,
        runtime: &BuiltRuntime,
        session: &str,
        allowed: Option<crate::cli_args::AllowedToolSet>,
    ) -> Option<Arc<dyn runtime::AsyncApiClient>> {
        let selected = self.selected.as_ref()?;
        let effort = selected.effort.as_deref().and_then(api::parse_effort_level);
        let Some(client) = super::runtime_builder::build_smart_live_client(
            runtime, session, allowed, &selected.model, effort, None,
        ) else {
            self.selected = None;
            self.receipt.held.push("client_unavailable".into());
            return None;
        };
        self.receipt.plan.model.clone_from(&selected.model);
        self.receipt.plan.effort.clone_from(&selected.effort);
        Some(client)
    }

    pub(crate) fn model(&self) -> &str { &self.receipt.plan.model }

    pub(crate) fn finish(mut self, cwd: &Path, session: &str, result: &Result<runtime::TurnSummary, String>) {
        self.receipt.at_ms = now_ms();
        self.receipt.duration_ms = self.receipt.at_ms.saturating_sub(self.began_ms);
        let records = runtime::read_route_outcomes(cwd).unwrap_or_default();
        let mut checked = false;
        let mut failed = result.is_err();
        for record in records.iter().filter(|record| record.run_id.as_deref() == Some(self.receipt.attempt.as_str())
            && record.decision_kind() == DecisionKind::Verify && record.verdict_basis_kind() == VerdictBasis::Objective
            && record.verdict_subject_kind() == runtime::VerdictSubject::Work)
        {
            checked = true;
            failed |= record.status != runtime::OUTCOME_COMPLETED;
        }
        self.receipt.verified = checked.then_some(!failed);
        match result {
            Ok(summary) => {
                if summary.assistant_messages.iter().any(|message| message.model.as_deref().is_none_or(|model|
                    api::resolve_model_alias(model) != api::resolve_model_alias(&self.receipt.plan.model))) {
                    self.receipt.held.push("work_model_changed_or_missing".into());
                }
                if summary.auto_compaction.is_some() || summary.budget_exhausted.is_some() {
                    self.receipt.held.push("partial_or_compacted".into());
                }
                if summary.assistant_messages.iter().flat_map(|message| &message.blocks).any(|block| {
                    matches!(block, runtime::ContentBlock::ToolUse { name, .. }
                        if super::plain_session::is_spawn_family_tool(name))
                }) || records.iter().any(|record| record.parent_attempt.as_deref() == Some(self.receipt.attempt.as_str())) {
                    self.receipt.held.push("child_usage_not_settled".into());
                }
            }
            Err(_) => self.receipt.held.push("turn_failed_or_cancelled".into()),
        }
        if self.receipt.plan.shape != PlanShape::Solo.label() {
            self.receipt.held.push("shape_usage_not_settled".into());
        }
        match tools::measure_plan_usage(cwd, session, &self.receipt.attempt, &self.receipt.plan.model, self.began_ms) {
            Some(usage) => {
                if usage.work_effort != self.receipt.plan.effort {
                    self.receipt.held.push("wire_effort_changed".into());
                }
                self.receipt.total_tokens = Some(usage.tokens);
                self.receipt.total_usd = Some(usage.usd);
            }
            None => self.receipt.held.push("usage_incomplete".into()),
        }
        if self.receipt.duration_ms == 0 { self.receipt.held.push("clock_unknown".into()); }
        if self.receipt.held.is_empty() {
            for observed in records.iter().filter(|record| record.run_id.as_deref() == Some(self.receipt.attempt.as_str())
                && record.decision_kind() == DecisionKind::Verify
                && record.verdict_basis_kind() == VerdictBasis::Objective
                && record.verdict_subject_kind() == runtime::VerdictSubject::Work
                && api::resolve_model_alias(&record.selected_model) == api::resolve_model_alias(&self.receipt.plan.model)) {
                let mut stamped = observed.clone()
                    .with_role(Some(self.receipt.cohort.role.clone()))
                    .with_complexity(Some(self.receipt.cohort.complexity.clone()))
                    .with_risk(Some(self.receipt.cohort.risk.clone()))
                    .with_effort_level(self.receipt.plan.effort.clone());
                stamped.shape = Some(self.receipt.plan.shape.clone());
                let _ = runtime::record_route_outcome(cwd, &stamped);
            }
        }
        let _ = tools::record_plan_receipt(cwd, &self.receipt);
    }
}
