//! Plan candidates and their total-cost scores — the one place a turn's
//! `(model, effort, shape, verify)` alternatives stand side by side.
//!
//! Today the router picks a model per spawn, the effort band comes from the
//! difficulty, the host prelude decides whether to split, and the verifier is
//! chosen by attendance — four decisions in four places, none of which sees
//! the others' cost (`docs/design/zo-autonomous-routing-review-20260915.md`
//! §1.1). This module lays the alternatives out as candidates and prices each
//! one the same way: what deciding costs, what the work costs, what a model
//! switch rewrites, what splitting and merging costs, and what verifying and
//! retrying costs — in tokens, in money when every price is known, and in
//! time — next to how likely the plan is to end verified and how often it
//! would have to ask a person.
//!
//! Pure: every number comes in through [`PlanContext`], [`PlanPriors`], the
//! evidence and price lookups. Nothing here names a model, reads a file or
//! knows a provider; the tools layer builds the inputs from the catalog, the
//! outcome store and the prompt-cache ledger. Staying on the current model is
//! always a candidate, and an alternative wins only when it beats the best
//! same-model plan by a margin the evidence has earned. A person's pin is a
//! constraint on the candidate set, never a score.

use super::outcome::{PlanShape, CONFIDENT_DECISIVE_SAMPLES};
use super::policy::RouteTaskComplexity;
use super::tiering::ModelBand;

/// How a plan's result would be checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VerifyMode {
    /// A command, test or gate whose exit code settles it.
    Objective,
    /// A model judge (the workflow verdict schema).
    ModelJudge,
    /// Nobody checks; an attended turn's completion receipt asks once.
    None,
}

impl VerifyMode {
    /// Every mode, the one place the set is enumerated.
    pub const ALL: [VerifyMode; 3] = [Self::Objective, Self::ModelJudge, Self::None];

    /// Canonical label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Objective => "objective",
            Self::ModelJudge => "model-judge",
            Self::None => "none",
        }
    }
}

/// Which door a model change came through. The review found five roads that
/// change the model on the wire without passing the router
/// (`docs/design/zo-autonomous-routing-review-20260915.md` §1.3): a person's
/// `/model`, the quota fallback, the refusal fallback, the overload demotion,
/// and the deep gate's role legs. Each one now files the same scored row the
/// turn start files, tagged with the door, so the soak can say per door
/// whether the scorer would have gone the same way and what the switch
/// rewrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SwitchTrigger {
    /// The person named a model (`/model`). Their choice stands; the row
    /// says what the scorer would have done and what the switch cost.
    Person,
    /// The main model's quota wall: the turn moved to the cross-provider
    /// fallback client.
    Quota,
    /// A safety-classifier refusal: the turn moved to the refusal fallback.
    Refusal,
    /// A provider overload shed this tier: the turn moved one rung down.
    Starvation,
    /// The deep gate's PLAN leg ran on its own client.
    PlanLeg,
    /// The deep gate's VERIFY leg ran on a ranked verifier client.
    VerifyLeg,
    /// The Architect contract's implementer ran the EXEC leg.
    ExecLeg,
    /// The step effort governor moved the turn a rung on the same provider
    /// — heavier after two strong stuck steps at the effort cap, lighter
    /// after a run of routine steps on the floor
    /// (`conversation/step_effort.rs`, t-5633).
    Step,
}

impl SwitchTrigger {
    /// Every trigger, the one place the set is enumerated.
    pub const ALL: [SwitchTrigger; 8] = [
        Self::Person,
        Self::Quota,
        Self::Refusal,
        Self::Starvation,
        Self::PlanLeg,
        Self::VerifyLeg,
        Self::ExecLeg,
        Self::Step,
    ];

    /// Canonical label, as the shadow ledger's `trigger` column spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Quota => "quota",
            Self::Refusal => "refusal",
            Self::Starvation => "starvation",
            Self::PlanLeg => "plan-leg",
            Self::VerifyLeg => "verify-leg",
            Self::ExecLeg => "exec-leg",
            Self::Step => "step",
        }
    }

    /// The trigger a label names, `None` for anything else.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|trigger| trigger.as_str() == label.trim())
    }

    /// Whether the model it left could not have served: a wall, a refusal
    /// or a shed tier. The scorer then drops that model from the candidate
    /// set instead of holding it up as the stay bar — the special case the
    /// review describes as a forcibly shrunk set (§4.3).
    #[must_use]
    pub fn forced(self) -> bool {
        matches!(self, Self::Quota | Self::Refusal | Self::Starvation)
    }

    /// Whether a role leg borrowed the client for one sub-turn rather than
    /// the conversation itself changing model.
    #[must_use]
    pub fn is_leg(self) -> bool {
        matches!(self, Self::PlanLeg | Self::VerifyLeg | Self::ExecLeg)
    }
}

/// One model the planner may choose, as the caller read it off the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub band: ModelBand,
    /// Effort labels this model may run at for this task — already narrowed
    /// by the caller's effort band for the difficulty (that band is one table
    /// in the tools layer; this module does not own a second copy). Empty
    /// means the model runs at its default and the candidate carries `None`.
    pub efforts: Vec<String>,
}

/// One alternative: which model, at what effort, laid out how, checked how.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanCandidate {
    pub model: String,
    pub effort: Option<String>,
    pub shape: PlanShape,
    pub verify: VerifyMode,
}

/// A model's list price, USD per million tokens. Absent prices are absent —
/// an unpriced candidate is never treated as free.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
}

/// What the prompt-cache ledger knows about one model's warm prefix in this
/// session: how much is cached, how old the last hit is, and the TTL it was
/// written under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheState {
    pub model: String,
    pub cached_prefix_tokens: u64,
    pub age_secs: u64,
    pub ttl_secs: u64,
}

impl CacheState {
    /// Whether the prefix would still be read back now.
    #[must_use]
    pub fn live(&self) -> bool {
        self.cached_prefix_tokens > 0 && self.age_secs < self.ttl_secs
    }
}

/// What the outcome store knows about one `(model, effort, shape)` on this
/// route: verified passes and failures only (a bare completion is not a
/// pass), and the medians a priced estimate prefers over a prior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlanEvidence {
    pub verified_passes: u32,
    pub verified_failures: u32,
    pub median_duration_ms: Option<u64>,
    pub median_output_tokens: Option<u64>,
}

impl PlanEvidence {
    fn samples(self) -> u32 {
        self.verified_passes.saturating_add(self.verified_failures)
    }
}

/// What the outcome store says about one candidate: the verified verdicts
/// (`decision: verify`, a settled pass or failure about the work) and the run
/// medians, over the records that match the candidate's model always and its
/// effort and shape whenever the record carries them — a record written
/// before those columns existed is a wildcard on them, never a mismatch. The
/// pure half of the evidence lookup; the tools layer hands it the records it
/// already loads for the router and the canonical form of the model id.
#[must_use]
pub fn plan_evidence_from_records(
    records: &[super::outcome::RouteOutcomeRecord],
    candidate: &PlanCandidate,
    canonical: impl Fn(&str) -> String,
) -> Option<PlanEvidence> {
    use super::outcome::{DecisionKind, VerdictSubject};
    let model = canonical(&candidate.model);
    let shape = candidate.shape.label();
    let matching = records.iter().filter(|record| {
        canonical(&record.selected_model) == model
            && record
                .effort_level
                .as_deref()
                .is_none_or(|effort| Some(effort) == candidate.effort.as_deref())
            && record.shape.as_deref().is_none_or(|label| label == shape)
    });
    let mut evidence = PlanEvidence::default();
    let mut durations = Vec::new();
    let mut outputs = Vec::new();
    let mut seen = false;
    for record in matching {
        seen = true;
        match record.decision_kind() {
            DecisionKind::Verify if record.verdict_subject_kind() == VerdictSubject::Work => {
                match record.status.as_str() {
                    "completed" => evidence.verified_passes = evidence.verified_passes.saturating_add(1),
                    "failed" => evidence.verified_failures = evidence.verified_failures.saturating_add(1),
                    _ => {}
                }
            }
            kind if kind.is_bookkeeping() => continue,
            _ => {}
        }
        if let Some(duration) = record.duration_ms {
            durations.push(duration);
        }
        if record.output_tokens > 0 {
            outputs.push(record.output_tokens);
        }
    }
    if !seen {
        return None;
    }
    evidence.median_duration_ms = median(&mut durations);
    evidence.median_output_tokens = median(&mut outputs);
    Some(evidence)
}

fn median(values: &mut [u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        values[mid - 1].midpoint(values[mid])
    } else {
        values[mid]
    })
}

/// The numbers a candidate falls back to before evidence. The caller fills
/// them from the catalog's priors section for the task's complexity; nothing
/// in this module carries a default figure, so a prior lives in the catalog
/// table or not at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanPriors {
    /// First-try verified pass rate by band, before any sample.
    pub pass_top: f64,
    pub pass_second: f64,
    pub pass_rest: f64,
    /// One work turn at this complexity: output tokens, requests, wall time.
    pub output_tokens: u64,
    pub requests: u32,
    pub duration_ms: u64,
    /// One model-judge verification: its output and wall time.
    pub judge_output_tokens: u64,
    pub judge_duration_ms: u64,
    /// One objective check's wall time (its tokens are zero).
    pub objective_check_ms: u64,
    /// One classification call (probe or decomposition): output and wall time.
    pub classify_output_tokens: u64,
    pub classify_ms: u64,
    /// The brief a lane or a delegate reads instead of the parent's context.
    pub lane_brief_tokens: u64,
    /// The handoff note a switched-to model reads on top of the context.
    pub handover_tokens: u64,
}

impl PlanPriors {
    fn pass_for(&self, band: ModelBand) -> f64 {
        match band {
            ModelBand::Top => self.pass_top,
            ModelBand::Second => self.pass_second,
            ModelBand::Rest | ModelBand::Superseded => self.pass_rest,
        }
    }
}

/// The turn as the planner sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanContext<'a> {
    pub complexity: RouteTaskComplexity,
    /// The conversation's current wire model and effort — the stay candidate.
    pub current_model: &'a str,
    pub current_effort: Option<&'a str>,
    /// The prefix a model must read to do this turn (the conversation so far).
    pub context_tokens: u64,
    /// A model or effort the person named: the candidate set shrinks to it.
    pub pinned_model: Option<&'a str>,
    pub pinned_effort: Option<&'a str>,
    /// Whether a person is watching (an unverified plan then costs an ask,
    /// and an unattended plan is never left unverified).
    pub attended: bool,
    /// Whether a command, test or gate exists that can settle the result.
    pub objective_check_available: bool,
    /// How many independent slices a read-only pre-plan found — `None` when
    /// nothing looked, `0`/`1` when the work is coupled. Lanes exist only
    /// when this is two or more: difficulty alone never splits.
    pub independent_slices: Option<u32>,
    /// The widest fan-out the difficulty ladder allows (`fanout_width_for`).
    pub max_lanes: u32,
    /// How many verify rounds a failing plan may take (`completion_ceiling_for`).
    pub verify_ceiling: u32,
    /// The pass probability a plan needs to be preferred on cost at all.
    pub min_p_verified: f64,
    /// The fraction an alternative must undercut the best same-model plan by
    /// when its evidence is fully confident; at zero confidence the margin
    /// doubles.
    pub switch_margin: f64,
}

/// One term of a plan's cost: tokens always, money when the price was known.
/// A term nobody computed costs nothing and is priced (zero); only a term
/// whose price lookup failed is unpriced — so a total is unpriced exactly
/// when some real spending could not be priced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostTerm {
    pub tokens: u64,
    pub usd: Option<f64>,
}

impl Default for CostTerm {
    fn default() -> Self {
        Self::zero()
    }
}

impl CostTerm {
    fn zero() -> Self {
        Self { tokens: 0, usd: Some(0.0) }
    }

    fn add(self, other: Self) -> Self {
        Self {
            tokens: self.tokens.saturating_add(other.tokens),
            usd: match (self.usd, other.usd) {
                (Some(a), Some(b)) => Some(a + b),
                _ => None,
            },
        }
    }

    fn scale(self, factor: f64) -> Self {
        Self {
            tokens: to_tokens(count_f64(self.tokens) * factor),
            usd: self.usd.map(|usd| usd * factor),
        }
    }
}

/// A token or millisecond count as a float for pricing and scaling. Counts here are at most
/// a session's worth of tokens — far below `f64`'s exact-integer range — so
/// the conversion is exact in practice; the lint is answered once, here.
#[allow(clippy::cast_precision_loss)]
fn count_f64(tokens: u64) -> f64 {
    tokens as f64
}

/// The five terms of the review's cost equation — deciding, working, switching
/// (the cache rewrite and the handover), splitting and merging, verifying and
/// retrying — each on its own so a shadow record can say where the money went.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CostBreakdown {
    pub decide: CostTerm,
    pub work: CostTerm,
    pub switch: CostTerm,
    pub integrate: CostTerm,
    pub verify: CostTerm,
}

impl CostBreakdown {
    /// All five terms together.
    #[must_use]
    pub fn total(&self) -> CostTerm {
        self.decide
            .add(self.work)
            .add(self.switch)
            .add(self.integrate)
            .add(self.verify)
    }
}

/// What a plan is expected to cost and yield.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanEstimate {
    /// Probability the plan ends verified (within its retry ceiling).
    pub p_verified: f64,
    /// How much evidence stands behind `p_verified`: `0.0` is prior only,
    /// `1.0` is [`CONFIDENT_DECISIVE_SAMPLES`] or more verified samples.
    pub confidence: f64,
    /// Expected wall time to a verified end, retries included.
    pub t_verified_ms: u64,
    /// Expected number of times a person has to step in.
    pub interventions: f64,
    pub cost: CostBreakdown,
}

impl PlanEstimate {
    /// The plan's expected total in USD, when every term was priced.
    #[must_use]
    pub fn cost_usd(&self) -> Option<f64> {
        self.cost.total().usd
    }

    /// The plan's expected total in tokens.
    #[must_use]
    pub fn tokens(&self) -> u64 {
        self.cost.total().tokens
    }
}

/// A candidate with its estimate.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredPlan {
    pub candidate: PlanCandidate,
    pub estimate: PlanEstimate,
}

/// Why a plan was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceReason {
    /// The person named the model or effort; the best plan inside that pin.
    Pinned,
    /// The best plan keeps the current model (its cache and its reasoning).
    Stay,
    /// Another model beat the best same-model plan by the earned margin.
    SwitchByMargin,
    /// No plan reached the pass threshold; the least bad one.
    NoneFeasible,
}

/// The chosen plan: an index into the scored list and the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanChoice {
    pub index: usize,
    pub reason: ChoiceReason,
}

/// Every alternative worth pricing for this turn. Rules, all table-free:
/// the current model is always in; a pin shrinks the set to what it names;
/// each model runs at each effort the caller allowed it; solo is always a
/// shape, a delegate joins from Medium up, and lanes appear only when the
/// pre-plan found two or more independent slices (capped by the ladder's
/// width and by the slice count); an objective check is offered when one
/// exists, a model judge always, and no check only on an attended turn at
/// Small or below.
#[must_use]
pub fn plan_candidates(ctx: &PlanContext<'_>, models: &[ModelOption]) -> Vec<PlanCandidate> {
    let mut candidates = Vec::new();
    for model in models {
        if ctx.pinned_model.is_some_and(|pinned| pinned != model.id) {
            continue;
        }
        let efforts: Vec<Option<String>> = if model.efforts.is_empty() {
            vec![None]
        } else {
            model
                .efforts
                .iter()
                .filter(|effort| ctx.pinned_effort.is_none_or(|pinned| pinned == effort.as_str()))
                .map(|effort| Some(effort.clone()))
                .collect()
        };
        for effort in efforts {
            for shape in shapes_for(ctx) {
                for verify in verify_modes_for(ctx) {
                    candidates.push(PlanCandidate {
                        model: model.id.clone(),
                        effort: effort.clone(),
                        shape,
                        verify,
                    });
                }
            }
        }
    }
    candidates
}

fn shapes_for(ctx: &PlanContext<'_>) -> Vec<PlanShape> {
    let mut shapes = vec![PlanShape::Solo];
    if matches!(ctx.complexity, RouteTaskComplexity::Medium | RouteTaskComplexity::Large) {
        shapes.push(PlanShape::Delegate);
    }
    let slices = ctx.independent_slices.unwrap_or(0);
    if slices >= 2 {
        let widest = slices.min(ctx.max_lanes);
        for width in 2..=widest {
            shapes.push(PlanShape::Parallel { width });
            if ctx.complexity == RouteTaskComplexity::Large {
                shapes.push(PlanShape::HostPrelude { width });
            }
        }
    }
    shapes
}

fn verify_modes_for(ctx: &PlanContext<'_>) -> Vec<VerifyMode> {
    let mut modes = Vec::new();
    if ctx.objective_check_available {
        modes.push(VerifyMode::Objective);
    }
    modes.push(VerifyMode::ModelJudge);
    let small = matches!(
        ctx.complexity,
        RouteTaskComplexity::Trivial | RouteTaskComplexity::Small
    );
    if ctx.attended && small {
        modes.push(VerifyMode::None);
    }
    modes
}

/// Price one candidate. `band_of` is the candidate model's band, `evidence`
/// what the store knows about this exact `(model, effort, shape)`, `price` the
/// model's list price (or `None`), `cache` the session's warm prefixes.
#[must_use]
pub fn score_plan(
    candidate: &PlanCandidate,
    ctx: &PlanContext<'_>,
    priors: &PlanPriors,
    band_of: impl Fn(&str) -> ModelBand,
    evidence: impl Fn(&PlanCandidate) -> Option<PlanEvidence>,
    price: impl Fn(&str) -> Option<ModelPrice>,
    cache: &[CacheState],
) -> PlanEstimate {
    let evidence = evidence(candidate).unwrap_or_default();
    let band = band_of(&candidate.model);
    let model_price = price(&candidate.model);
    let output_tokens = evidence.median_output_tokens.unwrap_or(priors.output_tokens);
    let duration_ms = evidence.median_duration_ms.unwrap_or(priors.duration_ms);
    let requests = u64::from(priors.requests.max(1));
    let context = ctx.context_tokens;

    // First-try pass: the band's prior, pulled toward the verified record with
    // the router's own confidence ramp as the pseudo-count.
    let k = f64::from(CONFIDENT_DECISIVE_SAMPLES.max(1));
    let samples = f64::from(evidence.samples());
    let p_first = (priors.pass_for(band) * k + f64::from(evidence.verified_passes)) / (k + samples);
    let confidence = (samples / k).min(1.0);

    // --- work: the turn with its prefix already warm on this model.
    let work_read = context.saturating_mul(requests);
    let work = term(model_price, 0, work_read, 0, output_tokens);

    // --- switch: what the first request rewrites. Staying pays only the
    // growth since the last hit; a cold or other model rewrites the whole
    // context plus the handover note, and reads that note ever after.
    let warm = cache
        .iter()
        .find(|state| state.model == candidate.model && state.live())
        .map_or(0, |state| state.cached_prefix_tokens.min(context));
    let staying = candidate.model == ctx.current_model;
    let handover = if staying { 0 } else { priors.handover_tokens };
    let rewrite = context.saturating_sub(warm).saturating_add(handover);
    let switch = term(model_price, 0, handover.saturating_mul(requests), rewrite, 0);

    // --- decide + integrate: lanes read a brief, not the context; the parent
    // then reads their answers in one more request of its own.
    let (decide, integrate) = match candidate.shape {
        PlanShape::Solo => (CostTerm::zero(), CostTerm::zero()),
        PlanShape::Delegate | PlanShape::Parallel { .. } | PlanShape::HostPrelude { .. } => {
            let brief = priors.lane_brief_tokens;
            let lane = term(
                model_price,
                0,
                brief.saturating_mul(requests.saturating_sub(1)),
                brief,
                output_tokens,
            )
            .scale(f64::from(candidate.shape.lanes()));
            let synthesis = term(model_price, 0, context, 0, output_tokens);
            let decide = if matches!(candidate.shape, PlanShape::Delegate) {
                CostTerm::zero()
            } else {
                term(model_price, 0, 0, 0, priors.classify_output_tokens)
            };
            (decide, lane.add(synthesis))
        }
    };

    // --- verify + retry: each round checks once; a failed round repeats the
    // work warm and checks again, up to the ceiling. Nobody checking means no
    // retry and, attended, one ask.
    let ceiling = u64::from(ctx.verify_ceiling.max(1));
    let p_fail = (1.0 - p_first).clamp(0.0, 1.0);
    let (check, check_ms) = match candidate.verify {
        VerifyMode::Objective => (CostTerm::zero(), priors.objective_check_ms),
        VerifyMode::ModelJudge => (
            term(
                model_price,
                0,
                0,
                priors.lane_brief_tokens,
                priors.judge_output_tokens,
            ),
            priors.judge_duration_ms,
        ),
        VerifyMode::None => (CostTerm::zero(), 0),
    };
    let (expected_retries, p_verified, interventions) = if candidate.verify == VerifyMode::None {
        (0.0, p_first, if ctx.attended { 1.0 } else { 0.0 })
    } else {
        let power = |round: u64| p_fail.powi(i32::try_from(round).unwrap_or(i32::MAX));
        let retries: f64 = (1..ceiling).map(power).sum();
        let unresolved = power(ceiling);
        (retries, 1.0 - unresolved, unresolved)
    };
    let retry_round = work.add(check);
    let verify = check.add(retry_round.scale(expected_retries));

    // --- time: decide, then the work (lanes run at once, then a synthesis),
    // then each check and each retry.
    let decide_ms = if matches!(
        candidate.shape,
        PlanShape::Parallel { .. } | PlanShape::HostPrelude { .. }
    ) {
        priors.classify_ms
    } else {
        0
    };
    let work_ms = match candidate.shape {
        PlanShape::Solo => duration_ms,
        _ => duration_ms.saturating_add(duration_ms / requests),
    };
    let retry_ms = count_f64(duration_ms.saturating_add(check_ms)) * expected_retries;
    let t_verified_ms = decide_ms
        .saturating_add(work_ms)
        .saturating_add(check_ms)
        .saturating_add(to_tokens(retry_ms));

    PlanEstimate {
        p_verified,
        confidence,
        t_verified_ms,
        interventions,
        cost: CostBreakdown {
            decide,
            work,
            switch,
            integrate,
            verify,
        },
    }
}

/// Price a request shape on one model: uncached input, cached reads, cache
/// writes, output. Unpriced when the model has no price.
fn term(price: Option<ModelPrice>, input: u64, cache_read: u64, cache_write: u64, output: u64) -> CostTerm {
    let tokens = input
        .saturating_add(cache_read)
        .saturating_add(cache_write)
        .saturating_add(output);
    let usd = price.map(|price| {
        (count_f64(input) * price.input
            + count_f64(cache_read) * price.cache_read
            + count_f64(cache_write) * price.cache_write
            + count_f64(output) * price.output)
            / 1_000_000.0
    });
    CostTerm { tokens, usd }
}

/// A scaled count back to an integer: rounded, and zero for anything that is
/// not a positive finite number. The guard makes the cast safe, so the two
/// cast lints are answered here once.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_tokens(value: f64) -> u64 {
    if value.is_finite() && value > 0.0 {
        value.round() as u64
    } else {
        0
    }
}

/// Pick among scored plans. Feasible plans (pass probability at or above the
/// threshold) are ranked by expected cost — unpriced last, since unknown is
/// not free — then by time, then by tokens. The best same-model plan is the
/// bar: another model's plan is chosen only when it undercuts that bar by the
/// switch margin, widened as its confidence falls. Inside a pin the same
/// ranking applies without the bar. `None` for an empty list.
#[must_use]
pub fn choose_plan(scored: &[ScoredPlan], ctx: &PlanContext<'_>) -> Option<PlanChoice> {
    if scored.is_empty() {
        return None;
    }
    let feasible: Vec<usize> = (0..scored.len())
        .filter(|index| scored[*index].estimate.p_verified >= ctx.min_p_verified)
        .collect();
    let (pool, feasible_any) = if feasible.is_empty() {
        ((0..scored.len()).collect::<Vec<_>>(), false)
    } else {
        (feasible, true)
    };
    let best = |indices: &[usize]| -> Option<usize> {
        indices.iter().copied().min_by(|a, b| rank_key(&scored[*a]).partial_cmp(&rank_key(&scored[*b])).unwrap_or(std::cmp::Ordering::Equal))
    };
    let best_overall = best(&pool)?;
    if !feasible_any {
        return Some(PlanChoice { index: best_overall, reason: ChoiceReason::NoneFeasible });
    }
    if ctx.pinned_model.is_some() || ctx.pinned_effort.is_some() {
        return Some(PlanChoice { index: best_overall, reason: ChoiceReason::Pinned });
    }
    let same_model: Vec<usize> = pool
        .iter()
        .copied()
        .filter(|index| scored[*index].candidate.model == ctx.current_model)
        .collect();
    let Some(best_stay) = best(&same_model) else {
        return Some(PlanChoice { index: best_overall, reason: ChoiceReason::SwitchByMargin });
    };
    if scored[best_overall].candidate.model == ctx.current_model {
        return Some(PlanChoice { index: best_overall, reason: ChoiceReason::Stay });
    }
    let alternative = &scored[best_overall].estimate;
    let stay = &scored[best_stay].estimate;
    let earned = match (alternative.cost_usd(), stay.cost_usd()) {
        (Some(alt), Some(bar)) => {
            let margin = ctx.switch_margin * (2.0 - alternative.confidence);
            alt <= bar * (1.0 - margin)
        }
        // A switch nobody can price does not clear the bar.
        _ => false,
    };
    if earned {
        Some(PlanChoice { index: best_overall, reason: ChoiceReason::SwitchByMargin })
    } else {
        Some(PlanChoice { index: best_stay, reason: ChoiceReason::Stay })
    }
}

/// Cheaper first; a missing price sorts after every known one.
fn rank_key(plan: &ScoredPlan) -> (u8, f64, u64, u64) {
    let estimate = &plan.estimate;
    match estimate.cost_usd() {
        Some(usd) => (0, usd, estimate.t_verified_ms, estimate.tokens()),
        None => (1, 0.0, estimate.t_verified_ms, estimate.tokens()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priors() -> PlanPriors {
        PlanPriors {
            pass_top: 0.9,
            pass_second: 0.8,
            pass_rest: 0.7,
            output_tokens: 4_000,
            requests: 4,
            duration_ms: 60_000,
            judge_output_tokens: 20_000,
            judge_duration_ms: 90_000,
            objective_check_ms: 30_000,
            classify_output_tokens: 500,
            classify_ms: 8_000,
            lane_brief_tokens: 3_000,
            handover_tokens: 400,
        }
    }

    fn ctx(current: &str) -> PlanContext<'_> {
        PlanContext {
            complexity: RouteTaskComplexity::Medium,
            current_model: current,
            current_effort: Some("high"),
            context_tokens: 50_000,
            pinned_model: None,
            pinned_effort: None,
            attended: false,
            objective_check_available: true,
            independent_slices: None,
            max_lanes: 4,
            verify_ceiling: 3,
            min_p_verified: 0.5,
            switch_margin: 0.15,
        }
    }

    const FLAT: ModelPrice = ModelPrice { input: 3.0, cache_read: 0.3, cache_write: 3.75, output: 15.0 };

    fn close(left: f64, right: f64) -> bool {
        (left - right).abs() < 1e-9
    }

    fn model(id: &str, band: ModelBand, efforts: &[&str]) -> ModelOption {
        ModelOption {
            id: id.to_string(),
            band,
            efforts: efforts.iter().map(|effort| (*effort).to_string()).collect(),
        }
    }

    fn solo(model: &str, effort: &str) -> PlanCandidate {
        PlanCandidate {
            model: model.to_string(),
            effort: Some(effort.to_string()),
            shape: PlanShape::Solo,
            verify: VerifyMode::Objective,
        }
    }

    #[test]
    fn a_pin_shrinks_the_candidate_set_to_what_the_person_named() {
        let models = [
            model("a", ModelBand::Top, &["high", "max"]),
            model("b", ModelBand::Second, &["high"]),
        ];
        let mut context = ctx("a");
        context.pinned_model = Some("b");
        let candidates = plan_candidates(&context, &models);
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|candidate| candidate.model == "b"));

        let mut context = ctx("a");
        context.pinned_effort = Some("max");
        let candidates = plan_candidates(&context, &models);
        assert!(candidates.iter().all(|candidate| candidate.effort.as_deref() == Some("max")));
        assert!(candidates.iter().all(|candidate| candidate.model == "a"));
    }

    #[test]
    fn lanes_come_from_independent_slices_never_from_difficulty_alone() {
        let models = [model("a", ModelBand::Top, &["high"])];
        let mut context = ctx("a");
        context.complexity = RouteTaskComplexity::Large;
        // Coupled (or unexamined) work: solo and delegate only.
        for slices in [None, Some(0), Some(1)] {
            context.independent_slices = slices;
            let shapes: std::collections::BTreeSet<_> = plan_candidates(&context, &models)
                .into_iter()
                .map(|candidate| candidate.shape.label())
                .collect();
            assert_eq!(
                shapes.into_iter().collect::<Vec<_>>(),
                vec!["delegate", "solo"],
                "{slices:?}"
            );
        }
        // Three independent slices under a ladder of four: widths two and three.
        context.independent_slices = Some(3);
        let shapes: std::collections::BTreeSet<_> = plan_candidates(&context, &models)
            .into_iter()
            .map(|candidate| candidate.shape.label())
            .collect();
        assert!(shapes.contains("parallel:2") && shapes.contains("parallel:3"));
        assert!(shapes.contains("host-prelude:2") && shapes.contains("host-prelude:3"));
        assert!(!shapes.contains("parallel:4"));
        // Many slices, narrow ladder: the ladder caps the width.
        context.independent_slices = Some(9);
        context.max_lanes = 2;
        let widest = plan_candidates(&context, &models)
            .into_iter()
            .map(|candidate| candidate.shape.lanes())
            .max();
        assert_eq!(widest, Some(2));
    }

    #[test]
    fn an_unattended_turn_is_never_left_unverified_and_an_attended_small_one_may_ask() {
        let models = [model("a", ModelBand::Top, &["high"])];
        let mut context = ctx("a");
        context.complexity = RouteTaskComplexity::Small;
        context.attended = false;
        assert!(plan_candidates(&context, &models)
            .iter()
            .all(|candidate| candidate.verify != VerifyMode::None));
        context.attended = true;
        let unverified = plan_candidates(&context, &models)
            .into_iter()
            .find(|candidate| candidate.verify == VerifyMode::None)
            .expect("an attended small turn may skip the check");
        let estimate = score_plan(
            &unverified,
            &context,
            &priors(),
            |_| ModelBand::Top,
            |_| None,
            |_: &str| Some(FLAT),
            &[],
        );
        assert!(close(estimate.interventions, 1.0));
        context.complexity = RouteTaskComplexity::Medium;
        assert!(plan_candidates(&context, &models)
            .iter()
            .all(|candidate| candidate.verify != VerifyMode::None));
    }

    #[test]
    fn staying_on_a_warm_model_pays_only_the_growth_and_a_switch_rewrites_the_context() {
        let context = ctx("a");
        let warm = [CacheState { model: "a".into(), cached_prefix_tokens: 48_000, age_secs: 30, ttl_secs: 300 }];
        let stay = score_plan(&solo("a", "high"), &context, &priors(), |_| ModelBand::Top, |_| None, |_: &str| Some(FLAT), &warm);
        let switch = score_plan(&solo("b", "high"), &context, &priors(), |_| ModelBand::Top, |_| None, |_: &str| Some(FLAT), &warm);
        // Stay rewrites the 2,000 tokens of growth; the switch rewrites the
        // whole 50,000 plus the handover note and reads the note each request.
        assert_eq!(stay.cost.switch.tokens, 2_000);
        assert_eq!(switch.cost.switch.tokens, 50_400 + 400 * 4);
        assert!(switch.cost_usd().unwrap() > stay.cost_usd().unwrap());
        // The same work term on both: the switch cost is isolated, not smeared.
        assert_eq!(stay.cost.work, switch.cost.work);
        // An expired prefix is a cold one.
        let expired = [CacheState { model: "a".into(), cached_prefix_tokens: 48_000, age_secs: 301, ttl_secs: 300 }];
        let cold = score_plan(&solo("a", "high"), &context, &priors(), |_| ModelBand::Top, |_| None, |_: &str| Some(FLAT), &expired);
        assert_eq!(cold.cost.switch.tokens, 50_000);
    }

    #[test]
    fn the_breakdown_sums_to_the_total_and_an_unpriced_model_has_no_total() {
        let context = ctx("a");
        let priced = score_plan(&solo("a", "high"), &context, &priors(), |_| ModelBand::Top, |_| None, |_: &str| Some(FLAT), &[]);
        let total = priced.cost.total();
        let by_hand = [priced.cost.decide, priced.cost.work, priced.cost.switch, priced.cost.integrate, priced.cost.verify]
            .iter()
            .fold(CostTerm::zero(), |acc, term| acc.add(*term));
        assert_eq!(total, by_hand);
        assert!(total.usd.is_some());
        let unpriced = score_plan(&solo("a", "high"), &context, &priors(), |_| ModelBand::Top, |_| None, |_| None, &[]);
        assert_eq!(unpriced.cost_usd(), None);
        assert_eq!(unpriced.tokens(), priced.tokens());
    }

    #[test]
    fn verified_evidence_moves_the_pass_rate_and_the_ceiling_bounds_the_retries() {
        let context = ctx("a");
        let prior_only = score_plan(&solo("a", "high"), &context, &priors(), |_| ModelBand::Rest, |_| None, |_: &str| Some(FLAT), &[]);
        assert!(close(prior_only.confidence, 0.0));
        // Rest prior 0.7, three rounds: 1 - 0.3^3.
        assert!((prior_only.p_verified - (1.0 - 0.3f64.powi(3))).abs() < 1e-9);
        let failing = PlanEvidence { verified_passes: 1, verified_failures: 7, ..PlanEvidence::default() };
        let learned = score_plan(&solo("a", "high"), &context, &priors(), |_| ModelBand::Rest, |_| Some(failing), |_: &str| Some(FLAT), &[]);
        assert!(close(learned.confidence, 1.0));
        assert!(learned.p_verified < prior_only.p_verified);
        assert!(learned.t_verified_ms > prior_only.t_verified_ms);
        assert!(learned.cost.verify.tokens > prior_only.cost.verify.tokens);
        // Expected retries never exceed ceiling - 1, however bad the record.
        let hopeless = PlanEvidence { verified_passes: 0, verified_failures: 100, ..PlanEvidence::default() };
        let bounded = score_plan(&solo("a", "high"), &context, &priors(), |_| ModelBand::Rest, |_| Some(hopeless), |_: &str| Some(FLAT), &[]);
        assert!(bounded.cost.work.tokens + bounded.cost.verify.tokens > 0);
        assert!(bounded.t_verified_ms <= priors().duration_ms * 3 + priors().objective_check_ms * 3 + 1);
    }

    #[test]
    fn lanes_pay_a_decide_call_briefs_and_a_synthesis_and_take_longer_than_a_solo() {
        let mut context = ctx("a");
        context.complexity = RouteTaskComplexity::Large;
        context.independent_slices = Some(2);
        let split = PlanCandidate { shape: PlanShape::Parallel { width: 2 }, ..solo("a", "high") };
        let alone = solo("a", "high");
        let s = score_plan(&split, &context, &priors(), |_| ModelBand::Top, |_| None, |_: &str| Some(FLAT), &[]);
        let a = score_plan(&alone, &context, &priors(), |_| ModelBand::Top, |_| None, |_: &str| Some(FLAT), &[]);
        assert_eq!(a.cost.decide.tokens, 0);
        assert_eq!(a.cost.integrate.tokens, 0);
        assert_eq!(s.cost.decide.tokens, 500);
        assert!(s.cost.integrate.tokens > 0);
        // Splitting is not free: a plan that splits for no reason loses on cost.
        assert!(s.cost_usd().unwrap() > a.cost_usd().unwrap());
        assert!(s.t_verified_ms > a.t_verified_ms);
    }

    #[test]
    fn stay_is_the_bar_and_an_alternative_needs_the_earned_margin() {
        let context = ctx("a");
        let scored = |alt_usd: f64, alt_confidence: f64| {
            let mk = |model: &str, usd: f64, confidence: f64| ScoredPlan {
                candidate: solo(model, "high"),
                estimate: PlanEstimate {
                    p_verified: 0.9,
                    confidence,
                    t_verified_ms: 1,
                    interventions: 0.0,
                    cost: CostBreakdown {
                        work: CostTerm { tokens: 1, usd: Some(usd) },
                        ..CostBreakdown::default()
                    },
                },
            };
            vec![mk("a", 1.0, 1.0), mk("b", alt_usd, alt_confidence)]
        };
        // 10% cheaper with full confidence: under the 15% margin, stay.
        let choice = choose_plan(&scored(0.90, 1.0), &context).unwrap();
        assert_eq!(choice, PlanChoice { index: 0, reason: ChoiceReason::Stay });
        // 20% cheaper with full confidence: clears 15%, switch.
        let choice = choose_plan(&scored(0.80, 1.0), &context).unwrap();
        assert_eq!(choice, PlanChoice { index: 1, reason: ChoiceReason::SwitchByMargin });
        // 20% cheaper on prior alone: the margin doubles to 30%, stay.
        let choice = choose_plan(&scored(0.80, 0.0), &context).unwrap();
        assert_eq!(choice, PlanChoice { index: 0, reason: ChoiceReason::Stay });
        // The same alternative under a pin is simply the best inside the pin.
        let mut pinned = ctx("a");
        pinned.pinned_model = Some("b");
        let only_b: Vec<_> = scored(0.90, 0.0).into_iter().filter(|plan| plan.candidate.model == "b").collect();
        let choice = choose_plan(&only_b, &pinned).unwrap();
        assert_eq!(choice.reason, ChoiceReason::Pinned);
    }

    #[test]
    fn evidence_counts_verified_verdicts_about_the_work_and_treats_old_rows_as_wildcards() {
        use super::super::outcome::{DecisionKind, RouteOutcomeRecord, VerdictSubject, VERDICT_SIGNAL};
        let verdict = |status: &str| {
            RouteOutcomeRecord::new("main", "turn", "a", status)
                .with_signal(VERDICT_SIGNAL)
                .with_decision(DecisionKind::Verify)
        };
        let records = vec![
            // Two verified passes and one failure on this model, one of them
            // stamped with the effort and shape, the others older wildcards.
            verdict("completed").with_effort_level(Some("high".into())).with_shape(PlanShape::Solo),
            verdict("completed"),
            verdict("failed"),
            // A validator fault is about the validator, not the work.
            verdict("failed").with_verdict_subject(VerdictSubject::Validator),
            // A run row brings a duration and an output figure.
            RouteOutcomeRecord::new("subagent", "Explore", "a", "completed")
                .with_decision(DecisionKind::Model)
                .with_duration_ms(Some(30_000))
                .with_output_tokens(2_000),
            // A row at another effort is not this candidate's evidence.
            verdict("failed").with_effort_level(Some("low".into())),
            // A classify row on this model is bookkeeping.
            RouteOutcomeRecord::new("main", "probe", "a", "failed")
                .with_decision(DecisionKind::Classify)
                .with_output_tokens(9_999),
            // Another model entirely.
            verdict("failed").with_shape(PlanShape::Solo),
        ];
        let mut other = records;
        other.last_mut().unwrap().selected_model = "b".into();
        let evidence = plan_evidence_from_records(&other, &solo("a", "high"), str::to_string).unwrap();
        assert_eq!(evidence.verified_passes, 2);
        assert_eq!(evidence.verified_failures, 1);
        assert_eq!(evidence.median_duration_ms, Some(30_000));
        assert_eq!(evidence.median_output_tokens, Some(2_000));
        // A model the store never saw has no evidence at all, not empty evidence.
        assert_eq!(plan_evidence_from_records(&other, &solo("zzz", "high"), str::to_string), None);
        // A shape the store never saw on this model falls back to the wildcard rows.
        let split = PlanCandidate { shape: PlanShape::Parallel { width: 2 }, ..solo("a", "high") };
        let evidence = plan_evidence_from_records(&other, &split, str::to_string).unwrap();
        assert_eq!((evidence.verified_passes, evidence.verified_failures), (1, 1));
    }

    #[test]
    fn an_unpriced_alternative_never_clears_the_bar_and_sorts_last() {
        let context = ctx("a");
        let mk = |model: &str, usd: Option<f64>, p: f64| ScoredPlan {
            candidate: solo(model, "high"),
            estimate: PlanEstimate {
                p_verified: p,
                confidence: 1.0,
                t_verified_ms: 1,
                interventions: 0.0,
                cost: CostBreakdown { work: CostTerm { tokens: 1, usd }, ..CostBreakdown::default() },
            },
        };
        let scored = vec![mk("a", Some(1.0), 0.9), mk("b", None, 0.9)];
        assert_eq!(choose_plan(&scored, &context).unwrap(), PlanChoice { index: 0, reason: ChoiceReason::Stay });
        // Nothing feasible: the least bad plan, said so.
        let scored = vec![mk("a", Some(1.0), 0.2), mk("b", Some(0.5), 0.3)];
        assert_eq!(
            choose_plan(&scored, &context).unwrap(),
            PlanChoice { index: 1, reason: ChoiceReason::NoneFeasible }
        );
        assert_eq!(choose_plan(&[], &context), None);
    }

    /// The five doors have one vocabulary: every trigger round-trips its
    /// label, the three the model could not have served through are the
    /// forced ones, and the three legs are legs — the two predicates the
    /// shadow uses to shrink the candidate set and to read a row.
    #[test]
    fn switch_triggers_round_trip_and_the_forced_and_leg_sets_are_disjoint() {
        for trigger in SwitchTrigger::ALL {
            assert_eq!(SwitchTrigger::from_label(trigger.as_str()), Some(trigger));
            assert_eq!(SwitchTrigger::from_label(&format!(" {} ", trigger.as_str())), Some(trigger));
            assert!(!(trigger.forced() && trigger.is_leg()), "{trigger:?}");
        }
        assert_eq!(SwitchTrigger::from_label("model"), None);
        let forced: Vec<SwitchTrigger> = SwitchTrigger::ALL.into_iter().filter(|t| t.forced()).collect();
        assert_eq!(forced, vec![SwitchTrigger::Quota, SwitchTrigger::Refusal, SwitchTrigger::Starvation]);
        let legs: Vec<SwitchTrigger> = SwitchTrigger::ALL.into_iter().filter(|t| t.is_leg()).collect();
        assert_eq!(legs, vec![SwitchTrigger::PlanLeg, SwitchTrigger::VerifyLeg, SwitchTrigger::ExecLeg]);
        assert!(!SwitchTrigger::Person.forced() && !SwitchTrigger::Person.is_leg());
    }
}
