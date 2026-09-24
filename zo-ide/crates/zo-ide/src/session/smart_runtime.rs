use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use runtime::{AsyncApiClient, ExecContract, RouteTaskIntent};

use super::BuiltRuntime;
use crate::cli_args::AllowedToolSet;

#[derive(Clone, Copy)]
enum SmartClientRole {
    QuotaFallback,
    RefusalFallback,
    DeepLane,
    Exec,
}

struct SmartTurnRouting {
    quota_fallback_model: Option<String>,
    refusal_fallback_model: Option<String>,
    classifier_fallback: runtime::ClassifierFallback,
    quota_wait_band: Duration,
    escalation_model_override: Option<String>,
    deep_verify_model: Option<String>,
    deep_plan_model: Option<String>,
    deep_tier_only: bool,
    deep_tier_models: Vec<String>,
    exec_impl_model: Option<String>,
    exec_swap: bool,
    plan_first: bool,
    verify_intent: RouteTaskIntent,
}

impl SmartTurnRouting {
    fn for_turn(
        routing: tools::SmartTurnRouting,
        input: &str,
        assessment: tools::TurnProbeAssessment,
    ) -> Self {
        let contract_eligible = tools::turn_has_write_intent(input)
            || matches!(
                assessment.intent,
                RouteTaskIntent::Design | RouteTaskIntent::Implementation
            );
        Self {
            quota_fallback_model: routing.quota_fallback_model,
            refusal_fallback_model: routing.refusal_fallback_model,
            classifier_fallback: routing.classifier_fallback,
            quota_wait_band: routing.quota_wait_band,
            // PlainSession has no confidence-cascade state. Refresh this
            // set-or-clear slot so a reused runtime cannot carry an override
            // installed by another host path.
            escalation_model_override: None,
            deep_verify_model: routing.deep_verify_model,
            deep_plan_model: routing.deep_plan_model,
            deep_tier_only: routing.deep_tier_only,
            deep_tier_models: routing.deep_tier_models,
            exec_impl_model: contract_eligible
                .then_some(routing.exec_impl_model)
                .flatten(),
            exec_swap: routing.exec_swap.arms_for(assessment.complexity),
            plan_first: matches!(
                assessment.complexity,
                runtime::RouteTaskComplexity::Medium | runtime::RouteTaskComplexity::Large
            ),
            verify_intent: assessment.intent,
        }
    }
}

struct SmartTurnWiring {
    quota_fallback_client: Option<(Arc<dyn AsyncApiClient>, String)>,
    refusal_fallback_client: Option<(Arc<dyn AsyncApiClient>, String)>,
    classifier_fallback: runtime::ClassifierFallback,
    quota_wait_band: Duration,
    escalation_model_override: Option<String>,
    deep_verify_client: Option<(Arc<dyn AsyncApiClient>, String)>,
    deep_plan_client: Option<(Arc<dyn AsyncApiClient>, String)>,
    deep_tier_only: bool,
    deep_tier_models: Vec<String>,
    exec_contract: Option<ExecContract>,
    verify_intent: RouteTaskIntent,
}

impl SmartTurnWiring {
    fn resolve(
        routing: SmartTurnRouting,
        mut client_for: impl FnMut(&str, SmartClientRole) -> Option<Arc<dyn AsyncApiClient>>,
    ) -> Self {
        let quota_fallback_client = routing
            .quota_fallback_model
            .and_then(|model| {
                client_for(&model, SmartClientRole::QuotaFallback)
                    .map(|client| (client, model))
            });
        let refusal_fallback_client = routing
            .refusal_fallback_model
            .and_then(|model| {
                client_for(&model, SmartClientRole::RefusalFallback)
                    .map(|client| (client, model))
            });
        let deep_verify_client = routing
            .deep_verify_model
            .and_then(|model| {
                client_for(&model, SmartClientRole::DeepLane).map(|client| (client, model))
            });
        let deep_plan_client = routing.deep_plan_model.and_then(|model| {
            deep_verify_client
                .as_ref()
                .filter(|(_, verify_model)| verify_model == &model)
                .cloned()
                .or_else(|| {
                    client_for(&model, SmartClientRole::DeepLane)
                        .map(|client| (client, model))
                })
        });
        let exec_contract = routing.exec_impl_model.map(|impl_model| ExecContract {
            impl_client: routing
                .exec_swap
                .then(|| client_for(&impl_model, SmartClientRole::Exec))
                .flatten(),
            impl_model,
            plan_first: routing.plan_first,
        });
        Self {
            quota_fallback_client,
            refusal_fallback_client,
            classifier_fallback: routing.classifier_fallback,
            quota_wait_band: routing.quota_wait_band,
            escalation_model_override: routing.escalation_model_override,
            deep_verify_client,
            deep_plan_client,
            deep_tier_only: routing.deep_tier_only,
            deep_tier_models: routing.deep_tier_models,
            exec_contract,
            verify_intent: routing.verify_intent,
        }
    }

    fn install<C, T>(self, target: &mut runtime::ConversationRuntime<C, T>)
    where
        C: runtime::ApiClient,
        T: runtime::ToolExecutor,
    {
        target.set_quota_fallback_client(self.quota_fallback_client);
        target.set_refusal_fallback_client(self.refusal_fallback_client);
        target.set_classifier_fallback(self.classifier_fallback);
        target.set_quota_wait_band(self.quota_wait_band);
        target.set_escalation_model_override(self.escalation_model_override);
        target.set_deep_verify_client(self.deep_verify_client);
        target.set_deep_plan_client(self.deep_plan_client);
        target.set_deep_tier_only(self.deep_tier_only);
        target.set_deep_tier_models(self.deep_tier_models);
        target.set_exec_contract(self.exec_contract);
        target.set_verify_intent(self.verify_intent);
    }
}

/// What the turn's Smart installation hands back: who orchestrates the
/// turn's sub-agents, and — when `smart.plan.shadow` is on — everything the
/// plan scorer's shadow needs later, once the host's own decision is known.
pub(crate) struct SmartTurnInstalled {
    pub(crate) orchestration: tools::HostOrchestration,
    /// Shared with the switch observer installed on the runtime, which
    /// files a row for every model switch the turn makes.
    pub(crate) plan_shadow: Option<Arc<PlanShadowTurn>>,
    /// What the same observer saw of the route this turn: the first switch
    /// that unseated it, if any — read at the turn's end for the routing
    /// seat's label (t-5806, `tools::note_route_followed`).
    pub(crate) route_watch: Arc<RouteWatch>,
}

/// The routing seat's witness for one turn: whether a model switch unseated
/// the route the judgment took part in before the turn ended.
///
/// Fed by the switch observer, which sees every switch the runtime makes,
/// and read once when the turn ends. The first unseating switch is kept —
/// a turn whose quota wall moved it and whose person then moved it again
/// disagreed with its route at the wall — and a leg's borrowed client or the
/// step governor's rung leaves it untouched (`tools::route_unseated_by`).
#[derive(Default)]
pub(crate) struct RouteWatch(std::sync::Mutex<Option<runtime::SwitchTrigger>>);

impl RouteWatch {
    fn note(&self, trigger: runtime::SwitchTrigger) {
        if !tools::route_unseated_by(trigger) {
            return;
        }
        let mut held = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        held.get_or_insert(trigger);
    }

    /// The switch that unseated the route, taken so a second reading of the
    /// same turn finds nothing.
    pub(crate) fn taken(&self) -> Option<runtime::SwitchTrigger> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
    }
}

impl SmartTurnInstalled {
    /// The host's side of the turn, as the prelude takes it.
    pub(crate) fn host_turn(&self) -> HostTurn<'_> {
        HostTurn {
            policy: self.orchestration,
            plan_shadow: self.plan_shadow.as_deref(),
        }
    }
}

/// What the host brings to a turn's prelude: who orchestrates, and the plan
/// scorer's shadow inputs when the shadow is on.
#[derive(Clone, Copy)]
pub(crate) struct HostTurn<'a> {
    pub(crate) policy: tools::HostOrchestration,
    pub(crate) plan_shadow: Option<&'a PlanShadowTurn>,
}

/// The plan scorer's shadow inputs gathered at turn start, from the same
/// settings snapshot and the same provider probe the live routes used.
pub(crate) struct PlanShadowTurn {
    pub(crate) settings: tools::PlanShadowSettings,
    /// The connected models with the efforts this turn's band allows them.
    pub(crate) models: Vec<runtime::ModelOption>,
    /// Whether a cross-model VERIFY leg stands for this turn.
    pub(crate) verify_leg: bool,
    /// The turn's effort floor, as the ledgers spell it.
    pub(crate) effort: Option<String>,
    /// The effort the person pinned (a static `/effort`, no band) — a
    /// constraint on the candidate set, never a score.
    pub(crate) pinned_effort: Option<String>,
    pub(crate) cwd: PathBuf,
}

/// What one turn brings to its Smart install: the prompt, its assessment,
/// the effort it starts on (floor, band ceiling) and where its route facts
/// go (t-5872).
#[derive(Clone, Copy)]
pub(crate) struct SmartTurnInput<'a> {
    pub(crate) input: &'a str,
    pub(crate) assessment: tools::TurnProbeAssessment,
    pub(crate) turn_effort: (Option<api::EffortLevel>, Option<api::EffortLevel>),
    pub(crate) route_fact: &'a super::route_fact::RouteFactSender,
}

/// Install this turn's Smart routes and answer who orchestrates its
/// sub-agents (`smart.orchestration`), read from the same merged settings
/// snapshot so the two can never disagree.
pub(crate) fn install_smart_turn(
    runtime: &mut BuiltRuntime,
    cwd: &Path,
    session_id: &str,
    allowed_tools: Option<&AllowedToolSet>,
    turn: SmartTurnInput<'_>,
) -> SmartTurnInstalled {
    let SmartTurnInput {
        input,
        assessment,
        turn_effort,
        route_fact,
    } = turn;
    // The turn's opening fact for the status row (t-5872): the judged band
    // and the effort floor — already decided above, nothing asked again.
    let _ = route_fact.send(super::route_fact::RouteFact::turn_start(
        assessment,
        turn_effort.0,
    ));
    let (routing, inventory) = tools::smart_turn_routing_and_inventory_for(
        cwd,
        runtime.api_client().model(),
        assessment.complexity,
    );
    // The conversation anchor marker's TTL for this turn's requests — a
    // process-wide declaration like attendance, read from the same settings
    // roots. Sub-agent clients keep the short markers regardless.
    runtime::declare_conversation_anchor_ttl(tools::conversation_anchor_ttl_for(cwd));
    let orchestration = routing.orchestration;
    let plan_shadow = routing.plan.shadow.then(|| {
        Arc::new(plan_shadow_turn(
            routing.plan,
            &inventory,
            routing.deep_verify_model.is_some(),
            turn_effort,
            cwd,
        ))
    });
    // The step effort governor for this turn: the person's `smart.zoStepEffort`
    // word, the turn's effort as its base, the connected inventory's rungs
    // beside the main model. Resolved before the runtime is borrowed below,
    // like the routes.
    let step_effort = step_effort_config(
        cwd,
        &inventory,
        runtime.api_client().model(),
        assessment.complexity,
        turn_effort,
        route_fact.clone(),
    );
    let routing = SmartTurnRouting::for_turn(routing, input, assessment);
    let wiring = SmartTurnWiring::resolve(routing, |model, role| {
        let (named_effort, effort_band_ceiling) = match role {
            SmartClientRole::QuotaFallback | SmartClientRole::RefusalFallback => (None, None),
            SmartClientRole::DeepLane => (Some(api::EffortLevel::Xhigh), None),
            SmartClientRole::Exec => turn_effort,
        };
        super::runtime_builder::build_smart_live_client(
            runtime,
            session_id,
            allowed_tools.cloned(),
            model,
            named_effort,
            effort_band_ceiling,
        )
    });
    let reserved_edit_gate = wiring
        .exec_contract
        .as_ref()
        .is_some_and(ExecContract::exec_swap_enabled);
    let route_watch = Arc::new(RouteWatch::default());
    if let Some(inner) = runtime.try_runtime_mut() {
        wiring.install(inner);
        inner.set_reserved_edit_gate(reserved_edit_gate);
        // Set-or-cleared with the rest of the turn's routes: a runtime a
        // previous turn governed is not governed by a word since erased.
        inner.set_step_effort(step_effort);
        // Every switch this turn makes — the quota and refusal fallbacks,
        // the overload demotion, a deep-gate leg's client — files the same
        // scored row the turn start files, tagged with its door, and tells
        // the routing seat's witness whether the route stood (t-5806).
        // Set-or-cleared with the rest of the turn's routes: the observer
        // is this turn's, and holds this turn's witness.
        let observed_shadow = plan_shadow.as_ref().map(Arc::clone);
        let watch = Arc::clone(&route_watch);
        let session_id = session_id.to_string();
        let complexity = assessment.complexity;
        inner.set_switch_observer(Some(Arc::new(move |switch: &runtime::ModelSwitch| {
            watch.note(switch.trigger);
            if let Some(shadow) = observed_shadow.as_ref() {
                record_plan_switch(shadow, &session_id, complexity, switch);
            }
        }) as runtime::SwitchObserver));
    }
    SmartTurnInstalled {
        orchestration,
        plan_shadow,
        route_watch,
    }
}

/// The step effort governor for this turn
/// (docs/design/zo-step-effort-governor-20260921.md §5), or none: when the
/// person wrote `off`, when the merged settings cannot be read, or when the
/// turn carries no effort to govern (`--effort off`).
///
/// No word at all installs the table alone — it decides, its rows are filed,
/// nothing is applied and the seat is never asked. A written word is the
/// seat's mode: `shadow` asks and records, `on` applies, `auto` applies once
/// the seat's own ledger raised it (`tools::step_effort_raised`).
fn step_effort_config(
    cwd: &Path,
    inventory: &runtime::ModelInventory,
    main_model: &str,
    band: runtime::RouteTaskComplexity,
    turn_effort: (Option<api::EffortLevel>, Option<api::EffortLevel>),
    route_fact: super::route_fact::RouteFactSender,
) -> Option<runtime::StepEffortConfig> {
    let word = tools::step_effort_word(cwd)?;
    if !word.governs() {
        return None;
    }
    let (floor, ceiling) = turn_effort;
    let floor = floor?;
    let held = held_on(api::detect_provider_kind(main_model));
    let asks = asks_on(word, held);
    let raised = asks && tools::step_effort_raised(cwd);
    let seat: Option<Arc<dyn runtime::StepEffortSeat>> =
        asks.then(|| tools::StepSeat::open(cwd) as Arc<dyn runtime::StepEffortSeat>);
    // Rows land in the project's shadow ledger, and only for a runtime a host
    // armed for durable traces — the guard the plan shadow and the timing
    // ledger keep, so a crate test's turns never write into a person's home.
    let ledger_cwd = cwd.to_path_buf();
    let observer: runtime::StepEffortObserver = Arc::new(move |event: &runtime::StepEvent| {
        if runtime::durable_traces_armed() {
            let _ = tools::record_step_event(&ledger_cwd, event);
        }
        // The same decision, to the status row: a step that moved the wire
        // replaces the turn's opening fact (t-5872).
        if let runtime::StepEvent::Step(row) = event {
            if let Some(fact) = super::route_fact::RouteFact::step(row) {
                let _ = route_fact.send(Some(fact));
            }
        }
    });
    let (heavier_model, cross_top_model) = rung_neighbours(inventory, main_model);
    Some(runtime::StepEffortConfig {
        applies: held.is_none() && word.applies_with(raised),
        floor,
        ceiling,
        band,
        heavier_model,
        lighter_model: api::starvation_demotion_model(main_model).map(|model| api::resolve_model_alias(&model)),
        cross_top_model,
        seat,
        observer: Some(observer),
        held,
    })
}

/// Why an applying word is held back on Anthropic's wire — the ledger's
/// word for it.
pub(crate) const HELD_ANTHROPIC_CACHE_PREFIX: &str = "anthropic_cache_prefix";

/// What the governor may not apply on a provider's wire, whatever the word
/// says (docs/design/zo-step-effort-governor-20260921.md §6): a wire whose
/// prompt cache keys the conversation on the request's effort — the
/// provider catalog's fact (`api::ProviderKind::effort_keys_the_cache`,
/// Anthropic's), where one rung of thinking saved re-wrote the whole cached
/// conversation twice. On that wire the governor records and never moves a
/// request.
fn held_on(provider: api::ProviderKind) -> Option<&'static str> {
    provider.effort_keys_the_cache().then_some(HELD_ANTHROPIC_CACHE_PREFIX)
}

/// Whether the governor asks its seat: a word that asks, on a wire it may
/// move (t-6342). On a held wire the seat's answer could only ever be
/// recorded: on 2026-09-23 this machine's ledger held 1,645 of its 1,665
/// steps, and all 239 of the seat's questions (160 requests) were about a
/// held step (docs/design/jev-engineering-review-20260923.md §7). So the
/// table decides alone, files its held row, and nothing is asked.
fn asks_on(word: tools::StepEffortWord, held: Option<&'static str>) -> bool {
    word.asks() && held.is_none()
}

/// The rungs the governor may move the turn to, read off the connected
/// inventory's own bands (`model_router::tiering`): the same provider's model
/// in the band above the main model, and another provider's top model. The
/// rung below is the catalog's own `demotes_to` (`api::starvation_demotion_model`),
/// the one the overload escape already walks.
fn rung_neighbours(inventory: &runtime::ModelInventory, main_model: &str) -> (Option<String>, Option<String>) {
    let main_id = api::resolve_model_alias(main_model);
    let Some(main) = inventory
        .find(&main_id)
        .or_else(|| inventory.find(main_model))
    else {
        return (None, None);
    };
    let usable = |model: &&runtime::ModelDescriptor| {
        model.band() != runtime::ModelBand::Superseded && model.id() != main.id()
    };
    let heavier = inventory
        .models()
        .iter()
        .filter(usable)
        .filter(|model| model.provider() == main.provider() && model.band() < main.band())
        .max_by_key(|model| model.band())
        .map(|model| model.id().to_string());
    let cross_top = inventory
        .models()
        .iter()
        .filter(usable)
        .find(|model| model.provider() != main.provider() && model.band() == runtime::ModelBand::Top)
        .map(|model| model.id().to_string());
    (heavier, cross_top)
}

/// The scorer's shadow inputs for one turn (or for a `/model` between
/// turns), from the settings snapshot and the connected inventory the live
/// routes were read from.
fn plan_shadow_turn(
    settings: tools::PlanShadowSettings,
    inventory: &runtime::ModelInventory,
    verify_leg: bool,
    turn_effort: (Option<api::EffortLevel>, Option<api::EffortLevel>),
    cwd: &Path,
) -> PlanShadowTurn {
    let (floor, ceiling) = turn_effort;
    PlanShadowTurn {
        settings,
        models: tools::model_options_for(inventory, floor, ceiling),
        verify_leg,
        effort: floor.map(|level| level.label().to_string()),
        // A static effort has no band ceiling: the person pinned it.
        pinned_effort: (ceiling.is_none())
            .then(|| floor.map(|level| level.label().to_string()))
            .flatten(),
        cwd: cwd.to_path_buf(),
    }
}

/// The plan shape the host's prelude decision amounts to: a fan-out is a
/// host pre-analysis of that width, anything else leaves the model alone.
/// One mapping for the shadow row and for the shape a turn's verdicts carry.
pub(crate) fn plan_shape_of(decision: tools::HostPrelude) -> runtime::PlanShape {
    match decision {
        tools::HostPrelude::Fanout { width } => runtime::PlanShape::HostPrelude {
            width: u32::try_from(width).unwrap_or(u32::MAX),
        },
        tools::HostPrelude::ModelLed => runtime::PlanShape::Solo,
    }
}

/// What one shadow row is about: the plan (or the switch) as it happened,
/// gathered by the caller that has the runtime — or the switch event — in
/// hand, so the one builder below never has to borrow either.
struct PlanShadowSubject<'a> {
    /// The model the row is scored from: the turn's model, or the one a
    /// switch left.
    current_model: &'a str,
    /// The prefix the (new) model reads.
    context_tokens: u64,
    /// The attempt the row's requests bill to.
    attempt: Option<&'a str>,
    pinned_model: Option<&'a str>,
    /// The shape the turn ran; a switch is always solo.
    shape: runtime::PlanShape,
    actual: tools::PlanShadowActual,
    /// `None` for the turn-start row, the door for a switch row.
    trigger: Option<runtime::SwitchTrigger>,
    /// The refusal category a classifier's switch named (t-6747).
    category: Option<&'a str>,
}

/// Build and append one shadow row for `subject` from the shadow's inputs:
/// catalog priors, the models the door allows, the verified record, the
/// shared price table (t-4290), the warm-prefix table (P0-B). Log-only. A
/// runtime no host armed (`relocate_traces_out_of_tree`) writes nothing —
/// its rows would land in the person's real home, the same guard the timing
/// ledger keeps. An error writing the row is dropped: evidence is never a
/// reason to fail a turn.
fn file_plan_shadow(
    shadow: &PlanShadowTurn,
    session_id: &str,
    complexity: runtime::RouteTaskComplexity,
    subject: PlanShadowSubject<'_>,
) {
    if !runtime::durable_traces_armed() {
        return;
    }
    let Some(priors) = tools::plan_priors_for(complexity) else {
        return;
    };
    let attended = matches!(runtime::declared_attendance(), runtime::Attendance::Attended);
    let slices = match subject.shape {
        runtime::PlanShape::HostPrelude { width } => Some(width),
        _ => None,
    };
    let ctx = runtime::PlanContext {
        complexity,
        current_model: subject.current_model,
        current_effort: shadow.effort.as_deref(),
        context_tokens: subject.context_tokens,
        pinned_model: subject.pinned_model,
        pinned_effort: shadow.pinned_effort.as_deref(),
        attended,
        objective_check_available: runtime::detect_check_command().is_some(),
        independent_slices: slices,
        max_lanes: tools::fanout_width_for(complexity)
            .and_then(|width| u32::try_from(width).ok())
            .unwrap_or(1),
        verify_ceiling: u32::try_from(runtime::completion_ceiling_for(complexity)).unwrap_or(1),
        min_p_verified: f64::from(shadow.settings.min_pass_percent) / 100.0,
        switch_margin: f64::from(shadow.settings.switch_margin_percent) / 100.0,
    };
    let records = runtime::read_route_outcomes(&shadow.cwd).unwrap_or_default();
    let price = tools::model_price_for;
    let recorded_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    // What each model still holds warm for this session, as the prompt-cache
    // state file has it: the input that lets the scorer charge a switch for
    // the prefix it rewrites and credit a stay for the prefix it reads.
    let warm: Vec<runtime::PlanCacheState> = api::warm_prefixes_for_session(session_id)
        .into_iter()
        .map(|(model, prefix)| runtime::PlanCacheState {
            model,
            cached_prefix_tokens: u64::from(prefix.prefix_tokens),
            age_secs: recorded_at.saturating_sub(prefix.observed_at_unix_secs),
            ttl_secs: prefix.ttl_secs,
        })
        .collect();
    // A forced switch could not have stayed: the door narrows the set.
    let models = match subject.trigger {
        Some(trigger) => tools::switch_candidates(trigger, subject.current_model, &shadow.models),
        None => shadow.models.clone(),
    };
    let row = tools::build_plan_shadow(tools::PlanShadowInputs {
        session_id,
        attempt: subject.attempt,
        recorded_at,
        ctx,
        models: &models,
        priors: &priors,
        records: &records,
        price: &price,
        cache: &warm,
        actual: subject.actual,
        trigger: subject.trigger,
        category: subject.category,
    });
    let _ = tools::record_plan_shadow(&shadow.cwd, &row);
}

/// File the plan scorer's shadow row for this turn: the alternatives priced
/// beside the plan the host actually chose (`decision`, the current model
/// and effort, the verify leg).
pub(crate) fn record_plan_shadow_turn(
    runtime: &mut BuiltRuntime,
    shadow: &PlanShadowTurn,
    setup: &super::turn_harness::TurnSetup,
    decision: tools::HostPrelude,
    session_id: &str,
) {
    let current_model = runtime.api_client().model().to_string();
    let context_tokens = runtime
        .try_runtime_mut()
        .map_or(0, |inner| u64::try_from(inner.estimated_tokens()).unwrap_or(u64::MAX));
    let shape = plan_shape_of(decision);
    let verify = if shadow.verify_leg {
        runtime::VerifyMode::ModelJudge
    } else {
        runtime::VerifyMode::None
    };
    // The attempt this turn is about to be spent on — the same key its
    // request rows, timing rows and verdict rows will carry (the prelude runs
    // before the turn's first request leaves, like the routing probe).
    let attempt = runtime
        .try_runtime()
        .map(runtime::ConversationRuntime::next_attempt);
    file_plan_shadow(
        shadow,
        session_id,
        setup.assessment.complexity,
        PlanShadowSubject {
            current_model: &current_model,
            context_tokens,
            attempt: attempt.as_deref(),
            pinned_model: setup.orchestration.user_named_model,
            shape,
            actual: tools::PlanShadowActual {
                model: current_model.clone(),
                effort: shadow.effort.clone(),
                shape: shape.label(),
                verify: verify.as_str().to_string(),
            },
            trigger: None,
            category: None,
        },
    );
}

/// The row a switch files: scored from the model it left, with the context
/// the new model reads and the attempt its requests bill to; `actual` is
/// where the switch went. A leg's row prices the leg's cold start (a VERIFY
/// leg sends its prompt alone, so its context is small by design); a forced
/// switch is scored against the models that were left standing.
fn record_plan_switch(
    shadow: &PlanShadowTurn,
    session_id: &str,
    complexity: runtime::RouteTaskComplexity,
    switch: &runtime::ModelSwitch,
) {
    let verify = if switch.trigger == runtime::SwitchTrigger::VerifyLeg {
        runtime::VerifyMode::ModelJudge
    } else {
        runtime::VerifyMode::None
    };
    file_plan_shadow(
        shadow,
        session_id,
        complexity,
        PlanShadowSubject {
            current_model: &switch.from,
            context_tokens: switch.context_tokens,
            attempt: (!switch.attempt.is_empty()).then_some(switch.attempt.as_str()),
            pinned_model: None,
            shape: runtime::PlanShape::Solo,
            actual: tools::PlanShadowActual {
                model: switch.to.clone(),
                effort: None,
                shape: runtime::PlanShape::Solo.label(),
                verify: verify.as_str().to_string(),
            },
            trigger: Some(switch.trigger),
            category: switch.category.as_deref(),
        },
    );
}

/// `/model` between turns: the person's choice stands, and — when the
/// shadow is on — a row says what the scorer would have done from the model
/// they left and what the switch rewrites. Read from the same settings
/// snapshot a turn would use; the next turn's attempt key, since that turn's
/// first request is the one that pays the rewrite.
pub(crate) fn record_person_model_switch(
    runtime: &BuiltRuntime,
    cwd: &Path,
    session_id: &str,
    to: &str,
    turn_effort: (Option<api::EffortLevel>, Option<api::EffortLevel>),
) {
    let from = runtime.api_client().model().to_string();
    if from == to {
        return;
    }
    let complexity = runtime::RouteTaskComplexity::Unknown;
    let (routing, inventory) = tools::smart_turn_routing_and_inventory_for(cwd, &from, complexity);
    if !routing.plan.shadow {
        return;
    }
    let shadow = plan_shadow_turn(
        routing.plan,
        &inventory,
        routing.deep_verify_model.is_some(),
        turn_effort,
        cwd,
    );
    let Some(inner) = runtime.try_runtime() else {
        return;
    };
    let switch = runtime::ModelSwitch {
        trigger: runtime::SwitchTrigger::Person,
        from,
        to: to.to_string(),
        context_tokens: u64::try_from(inner.estimated_tokens()).unwrap_or(u64::MAX),
        attempt: inner.next_attempt(),
        category: None,
    };
    record_plan_switch(&shadow, session_id, complexity, &switch);
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;

    use runtime::message_stream::{BlockId, RenderBlock};
    use runtime::{ApiRequest, AssistantEvent, RuntimeError};
    use tokio::sync::mpsc;

    use super::*;

    struct StubAsyncClient;

    impl AsyncApiClient for StubAsyncClient {
        fn stream_async<'a>(
            &'a self,
            _request: ApiRequest,
            _render_tx: mpsc::Sender<RenderBlock>,
            _text_block_id: BlockId,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a,
            >,
        > {
            Box::pin(async { Err(RuntimeError::new("stub client must not stream")) })
        }
    }

    struct StubSyncClient;

    impl runtime::ApiClient for StubSyncClient {
        fn stream(
            &mut self,
            _request: ApiRequest,
        ) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Err(RuntimeError::new("stub client must not stream"))
        }
    }

    /// An applying word is held on Anthropic's wire and nowhere else.
    #[test]
    fn the_governor_is_held_on_anthropics_wire_and_applies_elsewhere() {
        assert_eq!(held_on(api::ProviderKind::Anthropic), Some(HELD_ANTHROPIC_CACHE_PREFIX));
        assert_eq!(held_on(api::ProviderKind::OpenAi), None);
        assert_eq!(held_on(api::detect_provider_kind("claude-opus-5")), Some(HELD_ANTHROPIC_CACHE_PREFIX));
        assert_eq!(held_on(api::detect_provider_kind("gpt-5.6-sol")), None);
    }

    /// A wire the governor may not move asks its seat nothing (t-6342):
    /// every one of the seat's 239 questions on this machine was about a step
    /// held on Anthropic's wire, recorded and never applied. The table still
    /// decides and files its held row; a word that asks asks only where the
    /// answer can be carried.
    #[test]
    fn a_held_wire_asks_the_seat_nothing() {
        use zerocode_core::jev::JevMode;
        let anthropic = held_on(api::detect_provider_kind("claude-opus-5"));
        let openai = held_on(api::detect_provider_kind("gpt-5.6-sol"));
        for mode in [JevMode::Shadow, JevMode::On, JevMode::Auto] {
            let word = tools::StepEffortWord::Set(mode);
            assert!(!asks_on(word, anthropic), "{mode:?} on Anthropic");
            assert!(asks_on(word, openai), "{mode:?} on OpenAI");
        }
        for word in [tools::StepEffortWord::Absent, tools::StepEffortWord::Set(JevMode::Off)] {
            assert!(!asks_on(word, openai), "{word:?}");
        }
    }

    /// The rungs beside the main model come from the inventory's own bands:
    /// the same provider's band above it, and another provider's top model.
    #[test]
    fn rung_neighbours_read_the_inventorys_bands_and_never_name_a_model() {
        use runtime::model_router::{EffortCeiling, ModelSource, TiersProvenance};
        use runtime::{ModelCapability, ModelDescriptor, ModelInventory, ModelTier};
        let deep = [ModelTier::Deep, ModelTier::Strong];
        let strong = [ModelTier::Balanced, ModelTier::Strong];
        let coding = [ModelCapability::Default, ModelCapability::ToolUse, ModelCapability::Coding];
        let model = |id: &str, provider: &str, family: &str, tiers: &[ModelTier], ceiling, rank| {
            ModelDescriptor::new(id, provider, family)
                .source(ModelSource::EnabledBuiltinProvider)
                .capabilities(coding)
                .tiers(tiers.iter().copied())
                .tiers_provenance(TiersProvenance::ProviderDeclared)
                .effort_ceiling(ceiling)
                .release_rank(rank)
        };
        let inventory = ModelInventory::new(
            "claude-opus-5",
            vec![
                model("claude-fable-5-1", "anthropic", "fable", &deep, EffortCeiling::Max, 60),
                model("claude-opus-5", "anthropic", "opus", &strong, EffortCeiling::Max, 50),
                model("claude-sonnet-5", "anthropic", "sonnet", &strong, EffortCeiling::Max, 45),
                model("gpt-6-astra", "openai", "astra", &deep, EffortCeiling::Ultra, 60),
                model("gpt-5.6-sol", "openai", "sol", &strong, EffortCeiling::Ultra, 56),
            ],
        );
        // From the second band, the band above is the provider's top.
        let (heavier, cross_top) = rung_neighbours(&inventory, "claude-opus-5");
        assert_eq!(heavier.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(cross_top.as_deref(), Some("gpt-6-astra"));
        // From the top there is nothing above on this provider.
        let (heavier, cross_top) = rung_neighbours(&inventory, "claude-fable-5-1");
        assert_eq!(heavier, None);
        assert_eq!(cross_top.as_deref(), Some("gpt-6-astra"));
        // A model the inventory does not hold has no neighbours.
        assert_eq!(rung_neighbours(&inventory, "nobody"), (None, None));
    }

    fn routing() -> SmartTurnRouting {
        let routing = tools::SmartTurnRouting {
            quota_fallback_model: Some("openai-latest".to_string()),
            refusal_fallback_model: Some("google-latest".to_string()),
            classifier_fallback: runtime::ClassifierFallback::Auto,
            quota_wait_band: Duration::from_secs(7 * 60),
            deep_verify_model: Some("google-latest".to_string()),
            deep_plan_model: Some("claude-opus-5".to_string()),
            deep_tier_only: true,
            deep_tier_models: vec![
                "claude-opus-5".to_string(),
                "openai-latest".to_string(),
                "claude-fable-5".to_string(),
                "google-latest".to_string(),
            ],
            exec_impl_model: Some("gpt-5.6-terra".to_string()),
            exec_swap: tools::SmartExecSwap::Always,
            orchestration: tools::HostOrchestration::Auto,
            plan: tools::PlanShadowSettings::default(),
        };
        SmartTurnRouting::for_turn(
            routing,
            "implement the requested change",
            tools::TurnProbeAssessment {
                complexity: runtime::RouteTaskComplexity::Large,
                intent: RouteTaskIntent::Implementation,
                provenance: runtime::RouteAssessmentProvenance::Deterministic,
                judged: false,
            },
        )
    }

    fn install(
        routing: SmartTurnRouting,
    ) -> runtime::ConversationRuntime<StubSyncClient, runtime::StaticToolExecutor> {
        let wiring =
            SmartTurnWiring::resolve(routing, |_, _| Some(Arc::new(StubAsyncClient)));
        let mut runtime = runtime::ConversationRuntime::new(
            runtime::Session::new(),
            StubSyncClient,
            runtime::StaticToolExecutor::new(),
            runtime::PermissionPolicy::new(runtime::PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        );
        wiring.install(&mut runtime);
        runtime
    }

    #[test]
    fn quota_fallback_setting_injects_client() {
        let runtime = install(routing());

        assert_eq!(runtime.quota_fallback_model(), Some("openai-latest"));
    }

    #[test]
    fn refusal_fallback_setting_injects_client() {
        let runtime = install(routing());

        assert_eq!(runtime.refusal_fallback_client_model(), Some("google-latest"));
    }

    #[test]
    fn quota_wait_band_setting_injects_duration() {
        let runtime = install(routing());

        assert_eq!(runtime.quota_wait_band(), Duration::from_secs(7 * 60));
    }

    #[test]
    fn escalation_setting_injects_model_override() {
        let mut routing = routing();
        routing.escalation_model_override = Some("claude-fable-5".to_string());
        let runtime = install(routing);

        assert_eq!(
            runtime.escalation_model_override(),
            Some("claude-fable-5")
        );
    }

    #[test]
    fn deep_verify_setting_injects_client() {
        let runtime = install(routing());

        assert_eq!(runtime.deep_verify_model(), Some("google-latest"));
    }

    #[test]
    fn deep_plan_setting_injects_client() {
        let runtime = install(routing());

        assert_eq!(runtime.deep_plan_model(), Some("claude-opus-5"));
    }

    #[test]
    fn deep_tier_settings_inject_pool_and_only_invariant() {
        let runtime = install(routing());
        let expected = [
            "claude-opus-5".to_string(),
            "openai-latest".to_string(),
            "claude-fable-5".to_string(),
            "google-latest".to_string(),
        ];

        assert_eq!(
            (runtime.deep_tier_only(), runtime.deep_tier_models()),
            (true, expected.as_slice())
        );
    }

    #[test]
    fn exec_setting_injects_contract() {
        let runtime = install(routing());

        assert_eq!(
            runtime.exec_contract().map(|contract| (
                contract.impl_model.clone(),
                contract.impl_client.is_some(),
                contract.plan_first,
            )),
            Some(("gpt-5.6-terra".to_string(), true, true))
        );
    }

    #[test]
    fn verify_intent_setting_injects_intent() {
        let runtime = install(routing());

        assert_eq!(runtime.verify_intent(), RouteTaskIntent::Implementation);
    }
}
