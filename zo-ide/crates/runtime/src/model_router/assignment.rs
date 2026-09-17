use super::inventory::{ModelDescriptor, ModelInventory};
use super::learned::LearnedSpecialtyHint;
use super::outcome::RouteOutcomeSummary;
use super::policy::{
    auto_selectors_for_role, effective_specialty_adjustment, implementation_route_model_allowed, provider_allowlisted,
    provider_anchor_adjustment, same_main_bonus, selector_matches_context, tier_match_bonus_for,
    ModelCapability, ModelTier, RouteFeedbackHint, RouteTaskComplexity, RouteTaskRisk, SmartPolicy,
    TiersProvenance, CAPABILITY_MATCH_BONUS,
};
use super::target::{BuiltinSubagentProfile, RouteRole, RoutingTarget, SubagentProfileId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventorySummary {
    pub main_model: String,
    pub usable_model_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentSource {
    Auto,
    MainFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutoAssignmentOptions {
    pub allow_cross_provider_diversity: bool,
    /// Providers the auto recommendation may propose (`smart.providerAllowlist`).
    /// Empty allows every provider — kept in lockstep with the live route's
    /// `RoutePolicyContext::provider_allowlist` so the `/smart` dashboard
    /// preview equals what the runtime actually routes to.
    pub provider_allowlist: Vec<String>,
    /// The Smart execution-contract flavor (`smart.policy`) the preview routes
    /// under — kept in lockstep with the live route's
    /// [`super::policy::RoutePolicyContext::policy`] so the dashboard shows
    /// the same Verifier ladder and implementation gate the runtime enforces.
    /// Type default `Classic` (byte-identical preview when not injected).
    pub policy: SmartPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetAssignment {
    pub target: RoutingTarget,
    pub selected_model: String,
    pub source: AssignmentSource,
    pub confidence: AssignmentConfidence,
    pub reason: String,
    pub audit: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoAssignmentPlan {
    pub inventory_summary: InventorySummary,
    pub assignments: Vec<TargetAssignment>,
}

/// Build a deterministic recommended setup for every built-in subagent profile.
///
/// This is intentionally pure and inventory-bound: it never introduces a model
/// id that is not already present in `inventory`.
#[must_use]
pub fn recommend_auto_assignments(inventory: &ModelInventory) -> AutoAssignmentPlan {
    recommend_auto_assignments_with_options(inventory, &AutoAssignmentOptions::default())
}

#[must_use]
pub fn recommend_auto_assignments_with_options(
    inventory: &ModelInventory,
    options: &AutoAssignmentOptions,
) -> AutoAssignmentPlan {
    recommend_auto_assignments_inner(inventory, options, None, &LearnedSpecialtyHint::disabled())
}

/// Same as [`recommend_auto_assignments_with_options`] but folds the durable
/// route-outcome history into each target's score, so the `/smart` dashboard's
/// auto preview reflects what actually performed — not just the static
/// model-name/recency prior. The feedback term is the SAME bounded,
/// confidence-weighted, model-specific adjustment the live router applies
/// (`policy::score_model_with_context`), keyed per target by its route key, so
/// the previewed model matches the model the runtime would route to once the
/// same history exists. Pass this only when feedback-informed auto is enabled.
#[must_use]
pub fn recommend_auto_assignments_with_feedback(
    inventory: &ModelInventory,
    options: &AutoAssignmentOptions,
    feedback: &RouteOutcomeSummary,
) -> AutoAssignmentPlan {
    recommend_auto_assignments_inner(inventory, options, Some(feedback), &LearnedSpecialtyHint::disabled())
}

/// Same as [`recommend_auto_assignments_with_feedback`], additionally folding
/// in Phase 6's learned-specialty hint — unlike route-outcome feedback (keyed
/// to one subagent's specific `route_key`), learned specialty is keyed by
/// (role, model) alone, so it applies to every subagent profile's role
/// uniformly. Exists mainly so the shared `effective_specialty_adjustment`
/// scorer term has a dashboard-side caller to prove parity against the live
/// router (see the `learned_specialty_blend_agrees_between_live_route_and_dashboard_scorer`
/// test); Phase 7 is expected to wire this into the real `/smart` dashboard.
#[must_use]
pub fn recommend_auto_assignments_with_learned_specialty(
    inventory: &ModelInventory,
    options: &AutoAssignmentOptions,
    feedback: Option<&RouteOutcomeSummary>,
    learned: &LearnedSpecialtyHint,
) -> AutoAssignmentPlan {
    recommend_auto_assignments_inner(inventory, options, feedback, learned)
}

fn recommend_auto_assignments_inner(
    inventory: &ModelInventory,
    options: &AutoAssignmentOptions,
    feedback: Option<&RouteOutcomeSummary>,
    learned: &LearnedSpecialtyHint,
) -> AutoAssignmentPlan {
    let assignments = BuiltinSubagentProfile::all()
        .iter()
        .copied()
        .map(|profile| assign_builtin_profile(inventory, profile, options, feedback, learned))
        .collect();
    AutoAssignmentPlan {
        inventory_summary: InventorySummary {
            main_model: inventory.main_model().to_string(),
            usable_model_count: inventory.models().len(),
        },
        assignments,
    }
}

/// Recommended per-role fallback models, one [`TargetAssignment`] per built-in
/// [`RouteRole`]. Used by the `/smart` dashboard so the Roles tab previews the
/// model the runtime would auto-route to for each role under default task context
/// (same capability/tier/freshness prefilter and scorer), instead of always
/// showing the main-model fallback. A role with no qualifying model surfaces the
/// main-model fallback — exactly as the live route does.
#[must_use]
pub fn recommend_role_fallbacks(
    inventory: &ModelInventory,
    options: &AutoAssignmentOptions,
) -> Vec<TargetAssignment> {
    recommend_role_fallbacks_with_learned_specialty(inventory, options, &LearnedSpecialtyHint::disabled())
}

/// Same as [`recommend_role_fallbacks`], additionally folding in Phase 6's
/// learned-specialty hint (role-keyed, so — unlike route-outcome feedback —
/// it DOES apply to role-fallback previews; see
/// [`recommend_auto_assignments_with_learned_specialty`]'s doc for why).
#[must_use]
pub fn recommend_role_fallbacks_with_learned_specialty(
    inventory: &ModelInventory,
    options: &AutoAssignmentOptions,
    learned: &LearnedSpecialtyHint,
) -> Vec<TargetAssignment> {
    RouteRole::all()
        .iter()
        .copied()
        .map(|role| assign_role_fallback(inventory, role, options, learned))
        .collect()
}

fn assign_role_fallback(
    inventory: &ModelInventory,
    role: RouteRole,
    options: &AutoAssignmentOptions,
    learned: &LearnedSpecialtyHint,
) -> TargetAssignment {
    // Role-fallback routes are NOT feedback-keyed: the live router and the spawn
    // recorder both file their outcome under `subagent:{inferred_type}` (never
    // `role:{role}`), so no `role:*` history ever exists to learn from. Keeping
    // this on the static prior is honest; the subagent previews (which DO have a
    // matching `subagent:{key}` history) carry the learned signal instead.
    // Learned SPECIALTY, unlike feedback, is role-keyed (not route_key-keyed),
    // so it legitimately applies here too.
    let hint = RouteFeedbackHint::disabled();
    match best_model_for_role(inventory, role, options, &hint, learned) {
        Some(model) => TargetAssignment {
            target: RoutingTarget::RoleFallback(role),
            selected_model: model.id().to_string(),
            source: if model.id() == inventory.main_model() && inventory.models().len() == 1 {
                AssignmentSource::MainFallback
            } else {
                AssignmentSource::Auto
            },
            confidence: confidence_for(model, role, inventory.models().len(), options.policy, learned),
            reason: role_reason_for(role, inventory),
            audit: Vec::new(),
        },
        None => main_fallback_assignment(RoutingTarget::RoleFallback(role), inventory),
    }
}

/// Honest main-model fallback for a target whose role has no qualifying model in
/// the usable pool — exactly what the runtime does when AUTO route selection
/// returns `None` and falls back to the main decision.
fn main_fallback_assignment(target: RoutingTarget, inventory: &ModelInventory) -> TargetAssignment {
    TargetAssignment {
        target,
        selected_model: inventory.main_model().to_string(),
        source: AssignmentSource::MainFallback,
        confidence: AssignmentConfidence::Low,
        reason: "no usable model meets this role's requirements; using main model".to_string(),
        audit: vec!["no model matched role requirements; main fallback".to_string()],
    }
}

fn assign_builtin_profile(
    inventory: &ModelInventory,
    profile: BuiltinSubagentProfile,
    options: &AutoAssignmentOptions,
    feedback: Option<&RouteOutcomeSummary>,
    learned: &LearnedSpecialtyHint,
) -> TargetAssignment {
    let role = profile.route_role();
    // Subagent routes record + look up their outcome under `subagent:{key}` (see
    // `agent_tools::spawn`), so key the preview's feedback the same way.
    let hint = feedback_hint_for_route_key(feedback, &format!("subagent:{}", profile.key()));
    match best_model_for_role(inventory, role, options, &hint, learned) {
        Some(model) => TargetAssignment {
            target: RoutingTarget::Subagent(SubagentProfileId::builtin(profile)),
            selected_model: model.id().to_string(),
            source: if model.id() == inventory.main_model() && inventory.models().len() == 1 {
                AssignmentSource::MainFallback
            } else {
                AssignmentSource::Auto
            },
            confidence: confidence_for(model, role, inventory.models().len(), options.policy, learned),
            reason: reason_for(profile, model, inventory, options),
            audit: audit_for(profile, model, inventory, options),
        },
        None => main_fallback_assignment(
            RoutingTarget::Subagent(SubagentProfileId::builtin(profile)),
            inventory,
        ),
    }
}

/// Build the per-target feedback hint from the outcome summary, keyed by route
/// key. Only `subagent:{key}` targets are keyed: route outcomes are recorded
/// solely under `subagent:*` (the spawn recorder and live router file role
/// routes there too, never `role:*`), so that is the only key with a real
/// history to match. Returns a disabled hint when no summary is supplied
/// (feedback-informed auto off) so the scorer term is a no-op — making the
/// feedback-aware and plain paths byte-identical without a history.
fn feedback_hint_for_route_key(
    feedback: Option<&RouteOutcomeSummary>,
    route_key: &str,
) -> RouteFeedbackHint {
    feedback.map_or_else(RouteFeedbackHint::disabled, |summary| {
        summary.feedback_hint_for_route_key(route_key)
    })
}

fn best_model_for_role<'a>(
    inventory: &'a ModelInventory,
    role: RouteRole,
    options: &AutoAssignmentOptions,
    feedback: &RouteFeedbackHint,
    learned: &LearnedSpecialtyHint,
) -> Option<&'a ModelDescriptor> {
    // Apply the SAME selector ladder the live route uses. A fallback selector
    // (e.g. Analysis+Strong after Analysis+Deep) only competes when no stricter
    // candidate exists, so the dashboard preview cannot silently demote a role
    // just because a lower-tier model has a high release rank or feedback score.
    // This preview has no per-task risk/complexity (it is the "under default
    // task context" recommendation — see this fn's callers), so it passes the
    // neutral `Unknown`/`Unknown` context: byte-identical to the single
    // Balanced-only Verifier/Reviewer ladder this always used before the
    // situational escalation existed.
    for selector in auto_selectors_for_role(
        role,
        RouteTaskRisk::default(),
        RouteTaskComplexity::default(),
        options.policy,
    ) {
        let selected = inventory
            .models()
            .iter()
            // Same tier-only exemption as the live route's candidate walk
            // (`policy::ranked_context_candidates`) — without it the
            // dashboard recommended a different model than the runtime
            // actually routes to at the exact tier boundary the learned
            // admission exists to cross. Default (Unknown) risk, matching
            // this preview's documented default-task-context contract.
            .filter(|model| {
                selector_matches_context(model, &selector)
                    || super::policy::learned_tier_exempt_from_selector(
                        model,
                        &selector,
                        role,
                        RouteTaskRisk::default(),
                        learned,
                    )
            })
            .filter(|model| {
                !matches!(role, RouteRole::Coding | RouteRole::Debugging)
                    || implementation_route_model_allowed(
                        model.id(),
                        RouteTaskComplexity::Unknown,
                        0,
                        options.policy,
                    )
            })
            // Same hard allowlist prefilter as the live AUTO route: a disallowed
            // provider is never proposed, and an allowlist that excludes everything
            // surfaces the main-model fallback — exactly what the runtime does.
            .filter(|model| provider_allowlisted(&options.provider_allowlist, model.provider()))
            .max_by_key(|model| {
                score_model_for_role(model, &selector, role, inventory, options, feedback, learned)
            });
        if selected.is_some() {
            return selected;
        }
    }
    None
}

/// Recommendation scorer. Shares the base capability/tier weighting and the
/// [`provider_anchor_adjustment`] with the live route scorer
/// (`policy::score_model_with_context`); combined with the shared prefilter in
/// [`best_model_for_role`], the dashboard's recommended model equals the model
/// the runtime routes to **under default task context**. Anchoring is referenced
/// to the **main model** for every role (not the worker model), matching the live
/// path, and honors `allowCrossProviderDiversity` for worker roles too. Live
/// routing additionally applies per-task context bonuses, per-task MEMBERSHIP
/// gates (a High/Critical-risk route refuses the learned rung admission
/// outright — this preview always passes the default `Unknown` risk), and,
/// when feedback-informed auto is enabled, a bounded per-model feedback hint,
/// so an individual live route may differ from this default-context preview.
fn score_model_for_role(
    model: &ModelDescriptor,
    selector: &super::selector::RoleSelector,
    role: RouteRole,
    inventory: &ModelInventory,
    options: &AutoAssignmentOptions,
    feedback: &RouteFeedbackHint,
    learned: &LearnedSpecialtyHint,
) -> i32 {
    let mut score = 0_i32;
    if selector.capability.is_some_and(|capability| model.has_capability(capability)) {
        score += CAPABILITY_MATCH_BONUS;
    }
    // Same tier weighting as the live scorer, INCLUDING the provenance
    // discount and the learned-admission par grant — the dashboard used flat
    // literals here, so a guessed-tier (Fallback, −80) rung member ranked 80
    // points higher in the preview than in the live route: more than the
    // whole margin of a close contest, and exactly the kind of silent
    // divergence the shared-constant rule exists to prevent.
    if selector.tier.is_some_and(|tier| model.has_tier(tier)) {
        score += tier_match_bonus_for(model.tiers_provenance_value());
    } else if super::policy::learned_tier_exempt_from_selector(
        model,
        selector,
        role,
        RouteTaskRisk::default(),
        learned,
    ) {
        score += tier_match_bonus_for(TiersProvenance::Learned);
    }
    if role == RouteRole::Fast && model.has_capability(ModelCapability::Fast) {
        score += 250;
    }
    if model.id() == inventory.main_model() {
        score += same_main_bonus(role);
    }
    score += effective_specialty_adjustment(role, model, learned);
    score += i32::try_from(model.release_rank_value().min(100)).unwrap_or(0);
    if let Some(reference) = inventory.find(inventory.main_model()) {
        let diversity_role = matches!(role, RouteRole::Verifier | RouteRole::Reviewer | RouteRole::Judge);
        score += provider_anchor_adjustment(model, reference, diversity_role, options.allow_cross_provider_diversity);
    }
    // Same bounded, model-specific outcome-feedback term the live router applies
    // (`policy::score_model_with_context`). A disabled hint contributes 0, so this
    // is inert unless the caller threaded a real history through.
    score + i32::from(feedback.bounded_adjustment_for(model.id()))
}

fn role_requirements(role: RouteRole) -> (ModelCapability, ModelTier) {
    match role {
        RouteRole::Default => (ModelCapability::Default, ModelTier::Balanced),
        RouteRole::Fast => (ModelCapability::Fast, ModelTier::Fast),
        RouteRole::Coding => (ModelCapability::Coding, ModelTier::Strong),
        RouteRole::Debugging => (ModelCapability::Debugging, ModelTier::Strong),
        RouteRole::Verifier | RouteRole::Reviewer => (ModelCapability::Verification, ModelTier::Balanced),
        RouteRole::Analysis | RouteRole::Research | RouteRole::Judge | RouteRole::Synthesizer => {
            (ModelCapability::Analysis, ModelTier::Deep)
        }
        RouteRole::Writing => (ModelCapability::Writing, ModelTier::Balanced),
        RouteRole::Design => (ModelCapability::Design, ModelTier::Balanced),
    }
}

fn confidence_for(
    model: &ModelDescriptor,
    role: RouteRole,
    model_count: usize,
    policy: SmartPolicy,
    learned: &LearnedSpecialtyHint,
) -> AssignmentConfidence {
    if model_count <= 1 {
        return AssignmentConfidence::Low;
    }
    // A learned rung admission IS a selector fit for confidence purposes —
    // the router genuinely treats the model as a rung member, on evidence at
    // full ramp. Without this clause the dashboard would recommend the
    // admitted model (the selection walk knows the exemption) while labeling
    // its own top recommendation Medium, as if the pick were a requirements
    // compromise rather than the highest-confidence signal the table has.
    if auto_selectors_for_role(
        role,
        RouteTaskRisk::default(),
        RouteTaskComplexity::default(),
        policy,
    )
    .iter()
    .any(|selector| {
        selector_matches_context(model, selector)
            || super::policy::learned_tier_exempt_from_selector(
                model,
                selector,
                role,
                RouteTaskRisk::default(),
                learned,
            )
    }) {
        AssignmentConfidence::High
    } else {
        let (capability, tier) = role_requirements(role);
        if model.has_capability(capability) || model.has_tier(tier) {
            AssignmentConfidence::Medium
        } else {
            AssignmentConfidence::Low
        }
    }
}

fn role_reason_for(role: RouteRole, inventory: &ModelInventory) -> String {
    if inventory.models().len() <= 1 {
        return "single usable model pool; using main model".to_string();
    }
    let fit = match role {
        RouteRole::Default => "balanced default fit",
        RouteRole::Fast => "fast/low-latency fit",
        RouteRole::Coding => "coding fit",
        RouteRole::Debugging => "debugging and tool-use fit",
        RouteRole::Verifier => "verification fit",
        RouteRole::Reviewer => "code review fit",
        RouteRole::Analysis => "analysis fit",
        RouteRole::Research => "research and analysis fit",
        RouteRole::Writing => "writing fit",
        RouteRole::Design => "design fit",
        RouteRole::Judge => "deep judgement fit",
        RouteRole::Synthesizer => "synthesis fit",
    };
    fit.to_string()
}

fn reason_for(
    profile: BuiltinSubagentProfile,
    model: &ModelDescriptor,
    inventory: &ModelInventory,
    options: &AutoAssignmentOptions,
) -> String {
    if inventory.models().len() <= 1 {
        return "single usable model pool; using main model".to_string();
    }
    let base = match profile {
        BuiltinSubagentProfile::GeneralPurpose => "balanced worker fit",
        BuiltinSubagentProfile::Explore => "fast/read-only exploration fit",
        BuiltinSubagentProfile::Plan => "deep reasoning fit",
        BuiltinSubagentProfile::Verification => "verification fit",
        BuiltinSubagentProfile::DeepResearch => "research and analysis fit",
        BuiltinSubagentProfile::CodeReviewer => "code review verification fit",
        BuiltinSubagentProfile::Debugger => "debugging and tool-use fit",
        BuiltinSubagentProfile::DataAnalyst => "analysis and structured output fit",
        BuiltinSubagentProfile::Refactor => "coding consistency fit",
        BuiltinSubagentProfile::FrontendDesign => "design and writing fit",
        BuiltinSubagentProfile::ZoGuide => "writing and guidance fit",
        BuiltinSubagentProfile::StatuslineSetup => "fast setup task fit",
    };
    if !verifier_group(profile) {
        return base.to_string();
    }
    // Verifier/reviewer diversity is referenced to the MAIN model, matching the
    // live route scorer (`policy::provider_anchor_adjustment`).
    let Some(reference) = inventory.find(inventory.main_model()) else {
        return base.to_string();
    };
    if reference.id() == model.id() {
        return format!("{base}; diversity not needed for selected fit");
    }
    if reference.provider() == model.provider() && reference.family() != model.family() {
        format!("{base}; same-provider family diversity")
    } else if reference.provider() != model.provider() && options.allow_cross_provider_diversity {
        format!("{base}; cross-provider diversity explicitly allowed")
    } else {
        format!("{base}; cross-provider diversity disabled by default")
    }
}

fn audit_for(
    profile: BuiltinSubagentProfile,
    model: &ModelDescriptor,
    inventory: &ModelInventory,
    options: &AutoAssignmentOptions,
) -> Vec<String> {
    let mut audit = Vec::new();
    if verifier_group(profile) {
        audit.push(if options.allow_cross_provider_diversity {
            "cross-provider diversity: allowed".to_string()
        } else {
            "cross-provider diversity: disabled by default".to_string()
        });
        if let Some(reference) = inventory.find(inventory.main_model()) {
            audit.push(format!("diversity reference: {}", reference.id()));
            if reference.provider() != model.provider() {
                audit.push("selected provider differs from main".to_string());
            } else if reference.family() != model.family() {
                audit.push("selected family differs from main".to_string());
            }
        }
    }
    audit
}

fn verifier_group(profile: BuiltinSubagentProfile) -> bool {
    matches!(profile, BuiltinSubagentProfile::Verification | BuiltinSubagentProfile::CodeReviewer)
}
