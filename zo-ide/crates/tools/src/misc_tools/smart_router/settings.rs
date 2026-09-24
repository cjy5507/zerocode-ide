use std::collections::BTreeMap;
use std::path::Path;

use runtime::{
    connected_model_inventory, default_config_home, route_model, route_model_fallback_candidates,
    FreshnessPolicy, LearnedSpecialtyHint, RoleOverride, RoleSelector, RouteAutoClassifierMode,
    RouteContextNeed, RouteFeedbackHint, RoutePolicyContext, RouteRequest, RouteRole,
    RouteTaskComplexity, RouteTaskRisk, RouteToolNeed, RouteVerificationNeed, RoutingTarget,
    SmartPolicy, SubagentProfileId,
};
use serde_json::Value;

use super::infer::{role_key, route_role_from_key};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepTierModelsSetting {
    pub models: Vec<String>,
    pub configured: bool,
}

/// Per-turn Smart routes consumed by a foreground session host.
///
/// Model ids are resolved here, beside the merged-settings reader and router;
/// provider clients remain a host concern because they carry the live
/// session's tool registry, auth route, and cache scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartTurnRouting {
    pub quota_fallback_model: Option<String>,
    /// The cross-provider model a doubly-refused turn is handed to (t-6269):
    /// the first connected candidate on another provider in the main model's
    /// [`api::refusal_fallback_candidates`] list. `None` when the lineup names
    /// no cross-provider candidate or none is connected. The host builds a
    /// client for it exactly as for the quota fallback.
    pub refusal_fallback_model: Option<String>,
    /// What the refusal ladder does at its model-switching rungs
    /// (`smart.classifierFallback`, t-6747; default `ask`).
    pub classifier_fallback: runtime::ClassifierFallback,
    pub quota_wait_band: std::time::Duration,
    pub deep_verify_model: Option<String>,
    pub deep_plan_model: Option<String>,
    pub deep_tier_only: bool,
    pub deep_tier_models: Vec<String>,
    pub exec_impl_model: Option<String>,
    pub exec_swap: SmartExecSwap,
    /// Whether the host may run a difficulty-driven pre-analysis fan-out
    /// before the model turn (`smart.orchestration`).
    pub orchestration: HostOrchestration,
    /// The plan scorer's shadow knobs for this turn (`smart.plan.*`), read
    /// from the same snapshot so the shadow row and the live routes never
    /// disagree about the settings they saw.
    pub plan: PlanShadowSettings,
}

/// Default number of ranked fallback candidates a smart route carries for
/// quota/rate-limit escape (`smart.fallbackCandidateLimit`). Unchanged from
/// the previous hardcoded literal — only promoted to a settings knob so it is
/// tunable without a rebuild; Phase 7 is expected to surface it on `/smart`.
pub(super) const DEFAULT_FALLBACK_CANDIDATE_LIMIT: usize = 2;

/// Default Phase 5 exploration cadence (`smart.explorationCadence`): a
/// `route_key`'s exploration slot fires every Nth recorded outcome
/// (`total_records_for_route_key % N == 0`). N=5 matches the routing plan's
/// "1-in-5 spawns" acceptance criterion — frequent enough that a genuinely
/// under-sampled rival accumulates decisive samples in a reasonable number
/// of turns, infrequent enough that exploration stays a minority of traffic
/// even while the incumbent's dominance is being contested.
pub(super) const DEFAULT_EXPLORATION_CADENCE: usize = 5;

/// Default `smart.headroomPenaltyThreshold` — the *remaining* quota-headroom
/// percent below which the router starts DEDUCTING a candidate's score (graded,
/// capped at the binary-cooldown penalty). 25%: leaves the healthy band
/// untouched while nudging AUTO off a provider whose 5h/7d window (or
/// recent-429 estimate) is running thin. Clamped to `1..=100` on read. Kept in
/// lockstep by the CLI's `cli_snapshot_defaults_match_tools_crate_runtime_defaults`.
pub(super) const DEFAULT_HEADROOM_PENALTY_THRESHOLD: u8 = 25;
/// Default `smart.plan.minPassPercent`: a plan below an even chance of a
/// verified end is not preferred on cost, whatever it costs.
pub const DEFAULT_PLAN_MIN_PASS_PERCENT: u8 = 50;
/// Default `smart.plan.switchMarginPercent`: another model must be this much
/// cheaper than the best same-model plan, with confident evidence, before a
/// switch's cache rewrite and lost reasoning are worth it. Doubles at zero
/// confidence.
pub const DEFAULT_PLAN_SWITCH_MARGIN_PERCENT: u8 = 15;

/// Default `smart.quotaWaitBandMinutes` — how close (minutes) to a quota
/// window's reset the runtime turn loop HOLDS on the main model instead of
/// falling back to another provider. 15 minutes: ride out a short subscription
/// throttle on the configured model, but fall back when the wall is hours off.
/// `0` disables the band (pure fallback). Kept in lockstep by the CLI's
/// `cli_snapshot_defaults_match_tools_crate_runtime_defaults`.
pub(super) const DEFAULT_QUOTA_WAIT_BAND_MINUTES: u64 = 15;

/// Who orchestrates a turn's sub-agents (`smart.orchestration`).
///
/// The host classifies every turn's difficulty; this decides what it may DO
/// with the verdict. The model always keeps its own delegation rubric (the
/// base prompt teaches `Agent`/`SpawnMultiAgent`/`Workflow`); the host and
/// the model never both spawn for one turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HostOrchestration {
    /// The host runs a parallel pre-analysis before the model turn when the
    /// difficulty says the work is broad enough to split (`Large`), or when
    /// the user asked for parallel work. The default.
    #[default]
    Auto,
    /// The host never spawns; the model decides alone.
    ModelLed,
    /// Like `ModelLed`, and nothing about the classification is surfaced.
    Off,
}

impl HostOrchestration {
    /// Parse `smart.orchestration`; absent or unrecognized values use the
    /// documented `auto` default.
    #[must_use]
    pub fn from_settings_value(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str).map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("model") => Self::ModelLed,
            Some(value) if value.eq_ignore_ascii_case("off") => Self::Off,
            _ => Self::Auto,
        }
    }

    /// Stable `smart.orchestration` settings value — the exact inverse of
    /// [`Self::from_settings_value`].
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ModelLed => "model",
            Self::Off => "off",
        }
    }
}

/// When the foreground Architect contract may swap an EXEC leg from the
/// session model to the routed implementer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SmartExecSwap {
    /// Swap every implementation-shaped turn: the deep session model plans and
    /// verifies, the routed implementer writes the code. Opt-in, not the
    /// default — see [`Self::Never`] for why.
    Always,
    /// Swap only the router's lowest implementation-capable complexity band,
    /// keeping everything else on the session model. The conservative opt-out
    /// for someone who wants the reserved model to do the real coding.
    Easy,
    /// Keep every EXEC leg on the session model. The default.
    ///
    /// `Always` used to be the default, justified by the contract carrying its
    /// own quality net: two failed implementer attempts escalate the EXEC leg
    /// back to the session model. That net was unreachable. Interactive turns
    /// run `max_attempts: 2`, the attempt loop is `1..=max`, and escalation
    /// requires `attempt > ARCHITECT_IMPL_ATTEMPTS` where that constant is
    /// also 2 — so the escalating attempt never ran, and every
    /// implementation-shaped turn was authored end to end by a model the
    /// router had classified as *not* premium, whichever model the user chose.
    ///
    /// Swapping is a real economic win when the net works, so the mode stays;
    /// it just no longer applies to people who never asked for it. Turn it on
    /// with `/smart execswap always`.
    #[default]
    Never,
}

impl SmartExecSwap {
    /// Parse `smart.execSwap`; absent or unrecognized values use the documented
    /// `never` default.
    #[must_use]
    pub fn from_settings_value(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str).map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("always") => Self::Always,
            Some(value) if value.eq_ignore_ascii_case("easy") => Self::Easy,
            _ => Self::Never,
        }
    }

    /// Stable `smart.execSwap` settings value — the exact inverse of
    /// [`Self::from_settings_value`], so the `/smart execswap` writer and the
    /// reader can never drift apart on spelling.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Easy => "easy",
            Self::Never => "never",
        }
    }

    /// Whether this mode arms the implementer client for the classified turn.
    #[must_use]
    pub fn arms_for(self, complexity: RouteTaskComplexity) -> bool {
        match self {
            Self::Always => true,
            // `Trivial` is the classifier's lowest implementation-capable
            // band (write intent is checked separately by the host). `Small`
            // is the next graded band, so it intentionally stays native.
            Self::Easy => complexity == RouteTaskComplexity::Trivial,
            Self::Never => false,
        }
    }
}

/// Phase 6 `smart.learnedSpecialty` mode. Default `Shadow` — see each
/// variant's doc; `apply.rs`'s `SmartRouteContext` decides what to compute
/// and inject based on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum LearnedSpecialtyMode {
    /// Never compute or inject the learned-specialty hint at all — routing
    /// behaves exactly as if Phase 6 did not exist (the cold-start seed
    /// alone decides specialty).
    Off,
    /// Compute the hint, but route with it EMPTY (c=0 — the real decision
    /// stays seed-only) while additionally recording whether injecting it
    /// for real WOULD have changed the pick (`learned-shadow-differs:<model>`
    /// on the route audit) — a soak period before flipping to `On`.
    #[default]
    Shadow,
    /// Inject the computed hint for real; the live route blends seed and
    /// learned per [`runtime::RoutePolicyContext::learned_specialty`]'s
    /// confidence ramp.
    On,
}

impl LearnedSpecialtyMode {
    fn from_settings_value(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str).map(str::trim) {
            Some("off") => Self::Off,
            Some("on") => Self::On,
            // "shadow", missing, or any unrecognized value: the documented
            // default — same fail-closed-to-default convention as
            // `RouteAutoClassifierMode::from_settings_value`.
            _ => Self::Shadow,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // each bool is an independent smart.* feature gate, not a state machine
pub(super) struct SmartRuntimeSettings {
    pub enabled: bool,
    pub allow_cross_provider_diversity: bool,
    /// Whether the always-on deep-gate VERIFY leg may cross to a different
    /// provider than the main model (`smart.verifyCrossProvider`, default
    /// true). Decoupled from `allow_cross_provider_diversity` on purpose: the
    /// global worker-diversity flag can be off while verify stays cross-model
    /// (the default), or verify can be pinned to the native provider while
    /// worker diversity stays on. The foreground session reads this through
    /// [`smart_turn_routing_for`].
    pub verify_cross_provider: bool,
    /// Whether a main-model turn that exhausts its subscription/quota window
    /// (`RateLimit` after the retry budget) auto-falls-back to an equivalent
    /// model on a different provider for that turn (`smart.quotaFallback`,
    /// default true). The foreground session resolves the different-provider
    /// peer through [`smart_turn_routing_for`].
    pub quota_fallback: bool,
    /// Minutes-to-reset within which the runtime turn loop HOLDS on the main
    /// model instead of falling back (`smart.quotaWaitBandMinutes`, default
    /// [`DEFAULT_QUOTA_WAIT_BAND_MINUTES`]; `0` disables). The foreground
    /// session installs it at every turn entry.
    pub quota_wait_band_minutes: u64,
    /// Providers the auto route may pick (`smart.providerAllowlist`); empty =
    /// all connected providers (the default). Explicit models, pins, and the
    /// main-model fallback are never constrained by this.
    pub provider_allowlist: Vec<String>,
    /// Ordered Architect PLAN/VERIFY pool (`smart.deepTierModels`). Missing or
    /// empty uses the built-in pool; a non-empty array replaces it.
    pub deep_tier_models: Vec<String>,
    pub deep_tier_models_configured: bool,
    pub feedback_informed_auto: bool,
    pub auto_classifier: RouteAutoClassifierMode,
    pub subagents: BTreeMap<String, RoleOverride>,
    pub roles: BTreeMap<String, RoleOverride>,
    /// Ranked fallback-candidate count for quota/rate-limit escape
    /// (`smart.fallbackCandidateLimit`, default [`DEFAULT_FALLBACK_CANDIDATE_LIMIT`]).
    pub fallback_candidate_limit: usize,
    /// Master switch for Phase 5 deterministic exploration (`smart.exploration`).
    /// On by default: without it a zero/thin-history model that already
    /// cleared every capability/tier prefilter can never accumulate outcome
    /// samples (an established incumbent's feedback bound outlives any lane
    /// window), the exact live-data problem this phase exists to fix. Still
    /// gated per-route by risk/role hard gates regardless of this flag.
    pub exploration: bool,
    /// Cadence divisor for Phase 5 exploration (`smart.explorationCadence`,
    /// default [`DEFAULT_EXPLORATION_CADENCE`]).
    pub exploration_cadence: usize,
    /// Phase 6 `smart.learnedSpecialty` mode (default [`LearnedSpecialtyMode::Shadow`]).
    pub learned_specialty: LearnedSpecialtyMode,
    /// Remaining-percent threshold below which low quota headroom starts
    /// deducting route score (`smart.headroomPenaltyThreshold`, default
    /// [`DEFAULT_HEADROOM_PENALTY_THRESHOLD`], clamped `1..=100`). Injected into
    /// `RoutePolicyContext::headroom_penalty_threshold` for the graded penalty.
    pub headroom_penalty_threshold: u8,
    /// The Smart execution-contract flavor (`smart.policy`, default
    /// `architect`; explicit `"classic"` or `ZO_SMART_POLICY=classic` opts
    /// out — see [`SmartPolicy::from_settings_value`]). Injected into
    /// `RoutePolicyContext::policy` so the implementation gate and the
    /// Verifier ladder enforce the contract on every smart-routed spawn.
    pub policy: SmartPolicy,
    /// When the foreground deep gate may swap EXEC legs to the routed
    /// implementer (`smart.execSwap`, default `always`). This does not affect
    /// spawn routing or verifier selection.
    pub exec_swap: SmartExecSwap,
    /// `smart.classifierFallback`: `off`, `ask` (the default) or `auto` — see
    /// [`runtime::ClassifierFallback`]. A word that is none of them reads as
    /// the default, like every other malformed smart value.
    pub classifier_fallback: runtime::ClassifierFallback,
    /// Who orchestrates a turn's sub-agents (`smart.orchestration`, default
    /// `auto`): the host may pre-analyse a `Large` turn in parallel before
    /// the model turn, or leave every spawn to the model.
    pub orchestration: HostOrchestration,
    /// The plan scorer's shadow knobs (`smart.plan.*`) — see [`PlanShadowSettings`].
    pub plan: PlanShadowSettings,
}

/// How a Jev use is set (`smart.decisionShadow`, `smart.rerankShadow`) — the
/// use table's modes. `off`, the default, sends nothing. `shadow` and `auto`
/// record the judgment beside the product's own answer and act on nothing.
/// `on` acts on a validated judgment — routing folds it through the existing
/// conservative fusion rules and falls back to the chat probe per task, recall
/// reads the judgment's graph-safe order — and every road back from a judgment
/// that did not arrive or did not check out is the answer the product already
/// had. Which words each switch takes is its row in
/// `zerocode_core::jev::JEV_USES`, not this file's.
pub type DecisionShadowMode = zerocode_core::jev::JevMode;

/// The settings key the routing judgment's mode is read from, under `smart`.
pub const DECISION_SHADOW_SETTING: &str = zerocode_core::jev::ROUTING.setting;

/// `smart.decisionShadow` from the settings `loader` merges; `None` when the
/// merged settings cannot be read. Reads that one key: the shadow asks nothing
/// else of the settings, so it pays for nothing else. A sending mode carries at
/// most [`runtime::RUBRIC_TASK_CHAR_CAP`] characters of each task.
#[must_use]
pub fn decision_shadow_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::ROUTING.mode_in(&root))
}

/// `smart.rerankShadow`: whether every recall's notes are put to a System One
/// judgment. Its own switch, not the decision shadow's, because it sends
/// something else off the machine — the vault's summaries rather than the
/// task's text — and consent to one is not consent to the other.
pub const RERANK_SHADOW_SETTING: &str = zerocode_core::jev::RECALL.setting;

/// `smart.rerankShadow` from the settings `loader` merges, on the same terms
/// as [`decision_shadow_mode_from`]. `on` is recall's apply stage: the
/// judgment's graph-safe order is the order a turn reads
/// (`rerank_shadow::settle`). `auto` still records, because nothing promotes
/// recall — the row's `promotes` says so.
#[must_use]
pub fn rerank_shadow_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::RECALL.mode_in(&root))
}

/// `smart.skillSearch`: whether the installed skills are ranked against the
/// task by a judgment. Its own switch, not the other two's, because it sends
/// something else off the machine — the name and description of every skill
/// this machine has installed — and consent to one is not consent to another.
pub const SKILL_SEARCH_SETTING: &str = zerocode_core::jev::SKILLS.setting;

/// `smart.skillSearch` from the settings `loader` merges, on the same terms
/// as [`decision_shadow_mode_from`]. `on`, and an `auto` this seat's own
/// evidence has raised, are its apply stage — and what they apply is the
/// prompt: the skill index comes out and the two tools go in its place
/// (`runtime::prompt`). `shadow` leaves the index where it is and records
/// what the search would have handed back.
#[must_use]
pub fn skill_search_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::SKILLS.mode_in(&root))
}

/// `smart.skillSuggestion`: whether the turn-start suggestion — the two-stage
/// question asked at every turn's boundary whether or not the agent
/// searches — is asked, and whether its answer is handed to the turn. Its
/// own switch beside [`SKILL_SEARCH_SETTING`] (t-6877): the two ask
/// different words of the same catalog, and a seat is one question judged
/// on its own rows.
pub const SKILL_SUGGESTION_SETTING: &str = zerocode_core::jev::SKILL_SUGGESTION.setting;

/// `smart.skillSuggestion` from the settings `loader` merges, on the same
/// terms as [`skill_search_mode_from`] — and, while nobody wrote a word for
/// it, the word written for the search (`zerocode_core::jev::JevUse::follows`,
/// t-6877 round 3): a person who turned the search off before the two were
/// split turned the suggestion off with it. `on`, and an `auto` this seat's
/// own evidence has raised, hand the turn a note naming the skill the
/// judgment chose; `shadow` and a recording `auto` write the row and hand the
/// turn nothing.
#[must_use]
pub fn skill_suggestion_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::SKILL_SUGGESTION.mode_in(&root))
}

/// `smart.jevCompaction`: whether the tool results a full compaction is about
/// to summarize away are put to a judgment first. Its own switch, because it
/// sends something else off the machine — the heads of a session's tool
/// calls and results — and consent to one is not consent to another.
pub const JEV_COMPACTION_SETTING: &str = zerocode_core::jev::COMPACTION.setting;

/// `smart.jevCompaction` from the settings `loader` merges, on the same
/// terms as [`decision_shadow_mode_from`]. `on`, and an `auto` this seat's
/// own evidence has raised, are its apply stage — the dropped blocks leave
/// the summary's input (`runtime::compaction_relevance`). `shadow` records
/// what would have been dropped and the summary reads what it read before.
#[must_use]
pub fn jev_compaction_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::COMPACTION.mode_in(&root))
}

/// `smart.jevPatchReview`: whether every patch an edit tool writes is put to
/// the four review questions (t-6203). Its own switch, because it sends
/// something else off the machine again — a patch's hunks and the newest
/// lines of the output the edit followed — and consent to one is not consent
/// to another.
pub const JEV_PATCH_REVIEW_SETTING: &str = zerocode_core::jev::PATCH_REVIEW.setting;

/// `smart.jevPatchReview` from the settings `loader` merges, on the same terms
/// as [`decision_shadow_mode_from`]. `on`, and an `auto` this seat's own
/// evidence has raised, add one line to the result of a patch the review did
/// not permit (`runtime::patch_review`); `shadow` asks beside the turn and
/// changes nothing the model reads.
#[must_use]
pub fn jev_patch_review_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::PATCH_REVIEW.mode_in(&root))
}

/// Mode of the completion-claim seat, read from the same merged settings as
/// every other zo Jev seat.
#[must_use]
pub fn jev_claim_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::CLAIM.mode_in(&root))
}

/// `smart.jevFilePick` from the same merged settings root as the other Jev
/// seats. The table's `recommended` word keeps a new seat in record-only mode.
pub const JEV_FILE_PICK_SETTING: &str = zerocode_core::jev::FILE_PICK.setting;

#[must_use]
pub fn jev_file_pick_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::FILE_PICK.mode_in(&root))
}

/// `smart.jevChallenger`: whether one eligible spawn in five asks a model
/// nobody has evidence for the same design the routed model is about to
/// carry out, and puts the two to a blind comparison (t-6263). Its own
/// switch, because it spends something else — a bounded request of the
/// person's own provider credentials on a model the router did not pick,
/// carrying the head of the task — and consent to one seat is not consent to
/// another. Read from the same merged root as every other Jev seat; `None`
/// when the settings cannot be read.
pub const JEV_CHALLENGER_SETTING: &str = zerocode_core::jev::CHALLENGER.setting;

#[must_use]
pub fn jev_challenger_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::CHALLENGER.mode_in(&root))
}

/// `smart.jevCommandGuard`: whether a shell command is put to the command
/// guard before it runs (t-6348), from the same merged root as the other Jev
/// seats — `None` when the settings cannot be read.
pub const JEV_COMMAND_GUARD_SETTING: &str = zerocode_core::jev::COMMAND_GUARD.setting;

#[must_use]
pub fn jev_command_guard_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::COMMAND_GUARD.mode_in(&root))
}

/// `smart.jevToolTextGuard`: whether a text a tool hands back is put to the
/// tool text guard before the model reads it (t-6348).
pub const JEV_TOOL_TEXT_GUARD_SETTING: &str = zerocode_core::jev::TOOL_TEXT_GUARD.setting;

#[must_use]
pub fn jev_tool_text_guard_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::TOOL_TEXT_GUARD.mode_in(&root))
}

/// `smart.agentTool`: whether an agent's own question — zo's `Jev` tool, `zo
/// jev ask|choose|score` — is put to a System One judgment (t-6040). Its own
/// switch, because it sends something else off the machine again: not the
/// product's words about a task, but whatever an agent chose to ask, and
/// consent to one is not consent to the other.
pub const AGENT_TOOL_SETTING: &str = zerocode_core::jev::AGENT_TOOL.setting;

/// `smart.agentTool` from the settings `loader` merges, on the same terms as
/// [`decision_shadow_mode_from`]. `on` hands the answer to the agent; `shadow`
/// asks, writes the row and hands over nothing; `off` sends nothing.
#[must_use]
pub fn agent_tool_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::AGENT_TOOL.mode_in(&root))
}

/// `smart.jevMentionRerank`: whether one page of the `@` popup or the
/// `/resume` list is put to a judgment as the person types. Its own switch,
/// because it sends something else off the machine — the head of the
/// sentence a person is writing, and the names on the page — and consent
/// to one is not consent to another.
pub const JEV_MENTION_RERANK_SETTING: &str = zerocode_core::jev::MENTION_RERANK.setting;

/// `smart.jevMentionRerank` from the settings `loader` merges, on the same
/// terms as [`decision_shadow_mode_from`]. `on`, and an `auto` this seat's
/// own labels have raised, are its apply stage — the page takes the
/// judgment's order while the selection still sits on its first row
/// (`mention_rerank`). `shadow` records what would have been put first and
/// the page stands.
#[must_use]
pub fn jev_mention_rerank_mode_from(loader: &runtime::ConfigLoader) -> Option<DecisionShadowMode> {
    merged_settings_root_from(loader).map(|root| zerocode_core::jev::MENTION_RERANK.mode_in(&root))
}

/// The plan scorer's knobs (`smart.plan.*`). While the scorer runs in shadow
/// it decides nothing; these only shape what the shadow ledger records, so a
/// later comparison reads the thresholds the live scorer would use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanShadowSettings {
    /// Whether a turn writes a plan-shadow row at all (`smart.plan.shadow`,
    /// default true). Off means no scoring work and no row.
    pub shadow: bool,
    /// The verified-pass probability a plan needs before it is preferred on
    /// cost (`smart.plan.minPassPercent`, default
    /// `DEFAULT_PLAN_MIN_PASS_PERCENT`, clamped `0..=100`).
    pub min_pass_percent: u8,
    /// How far another model must undercut the best same-model plan, with
    /// fully confident evidence (`smart.plan.switchMarginPercent`, default
    /// `DEFAULT_PLAN_SWITCH_MARGIN_PERCENT`, clamped `0..=100`); at zero
    /// confidence the margin doubles.
    pub switch_margin_percent: u8,
}

impl Default for PlanShadowSettings {
    fn default() -> Self {
        Self {
            shadow: true,
            min_pass_percent: DEFAULT_PLAN_MIN_PASS_PERCENT,
            switch_margin_percent: DEFAULT_PLAN_SWITCH_MARGIN_PERCENT,
        }
    }
}

impl PlanShadowSettings {
    /// Read the `smart.plan` object; every absent or malformed key keeps its
    /// default, and a percent is pinned into `0..=100` rather than dropped.
    fn from_settings_value(value: Option<&Value>) -> Self {
        let plan = value.and_then(Value::as_object);
        let percent = |key: &str, default: u8| {
            plan.and_then(|plan| plan.get(key))
                .and_then(Value::as_u64)
                .map(|value| value.min(100))
                .and_then(|value| u8::try_from(value).ok())
                .unwrap_or(default)
        };
        Self {
            shadow: plan
                .and_then(|plan| plan.get("shadow"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
            min_pass_percent: percent("minPassPercent", DEFAULT_PLAN_MIN_PASS_PERCENT),
            switch_margin_percent: percent("switchMarginPercent", DEFAULT_PLAN_SWITCH_MARGIN_PERCENT),
        }
    }
}

/// Plain-data snapshot of every `smart.*` runtime default this module's
/// reader (`read_smart_runtime_settings`) falls back to when a key is
/// absent from `settings.json`. Exposed publicly (re-exported at the crate
/// root as `tools::smart_setting_defaults`) ONLY so the CLI crate's
/// `SmartSettingsSnapshot` reader (`snapshot_from_root`) can assert its own
/// defaults stay byte-identical to this crate's — the two crates parse the
/// SAME `settings.json` independently for two different surfaces (the
/// dashboard preview vs. live routing), and the smart-auto routing plan
/// documents this as the dual-reader drift point (P7). Not consumed by any
/// routing path — read-only, for a cross-crate test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // each bool mirrors an independent smart.* feature gate default, not a state machine
pub struct SmartSettingDefaults {
    pub enabled: bool,
    pub allow_cross_provider_diversity: bool,
    /// Default for `smart.verifyCrossProvider` — the deep-gate VERIFY leg's own
    /// cross-provider switch (`true`, same as the global diversity flag).
    pub verify_cross_provider: bool,
    /// Default for `smart.quotaFallback` — the automatic cross-provider fallback
    /// on main-model quota exhaustion (`true`).
    pub quota_fallback: bool,
    /// Default `smart.quotaWaitBandMinutes` (`DEFAULT_QUOTA_WAIT_BAND_MINUTES`).
    pub quota_wait_band_minutes: u64,
    /// Default `smart.deepTierModels` preference order.
    pub deep_tier_models: &'static [&'static str],
    pub feedback_informed_auto: bool,
    pub fallback_candidate_limit: usize,
    pub exploration: bool,
    pub exploration_cadence: usize,
    /// `true` when the default `learnedSpecialty` mode is `shadow` (today's
    /// documented default) rather than `off`/`on`.
    pub learned_specialty_defaults_to_shadow: bool,
    /// Default `smart.headroomPenaltyThreshold` (`DEFAULT_HEADROOM_PENALTY_THRESHOLD`).
    pub headroom_penalty_threshold: u8,
    /// Default `smart.policy` (`Architect` — the role-separation contract is
    /// the live default; `classic` opts out).
    pub policy: SmartPolicy,
    /// Default `smart.execSwap` (`never` — see [`SmartExecSwap::Never`] for
    /// why `always` was demoted).
    pub exec_swap: SmartExecSwap,
    /// Default `smart.orchestration` (`auto`).
    pub orchestration: HostOrchestration,
}

#[must_use]
pub fn smart_setting_defaults() -> SmartSettingDefaults {
    SmartSettingDefaults {
        enabled: true,
        allow_cross_provider_diversity: true,
        verify_cross_provider: true,
        quota_fallback: true,
        quota_wait_band_minutes: DEFAULT_QUOTA_WAIT_BAND_MINUTES,
        deep_tier_models: api::orchestration_reserved_models(),
        feedback_informed_auto: true,
        fallback_candidate_limit: DEFAULT_FALLBACK_CANDIDATE_LIMIT,
        exploration: true,
        exploration_cadence: DEFAULT_EXPLORATION_CADENCE,
        learned_specialty_defaults_to_shadow: matches!(
            LearnedSpecialtyMode::default(),
            LearnedSpecialtyMode::Shadow
        ),
        headroom_penalty_threshold: DEFAULT_HEADROOM_PENALTY_THRESHOLD,
        policy: SmartPolicy::Architect,
        exec_swap: SmartExecSwap::Never,
        orchestration: HostOrchestration::Auto,
    }
}

pub(super) fn read_smart_runtime_settings() -> Option<SmartRuntimeSettings> {
    // Route policy follows the SAME merged settings the session resolved —
    // global settings.json, the project's .zo/settings*.json, and a
    // `--settings` overlay (ConfigLoader holds that flag as a process-wide
    // override). The old direct read of only the global file meant a project
    // or CLI-level `smart.enabled: false` was silently ignored by spawn
    // routing. Missing files still merge to an empty root (defaults ON); a
    // load failure bails to None (fail-safe: no routing), matching the old
    // malformed-file behavior.
    let cwd = std::env::current_dir().unwrap_or_else(|_| default_config_home());
    read_smart_runtime_settings_for(&cwd)
}

#[allow(clippy::too_many_lines)] // flat, one-block-per-key parser for the merged smart.* object
pub(super) fn read_smart_runtime_settings_for(cwd: &Path) -> Option<SmartRuntimeSettings> {
    let root = merged_settings_root(cwd)?;
    let smart = root.get("smart").and_then(Value::as_object);
    let enabled = smart
        .and_then(|smart| smart.get("enabled"))
        .and_then(Value::as_bool)
        // On by default: smart AUTO is the product's routing brain — subagent
        // spawns land on the best connected model per role (verify cross-checks
        // on a different provider, workers get best-of-breed) with zero setup.
        // Note the blast radius includes single-provider users too: the
        // inventory carries every catalog entry of a usable provider, so spawns
        // can land on a different same-provider model/effort tier. An explicit
        // `smart.enabled: false` (or `/smart off`) still wins. Keep in lockstep
        // with `snapshot_from_root` in the CLI and `smart_setting_defaults()`.
        .unwrap_or(true);
    let allow_cross_provider_diversity = smart
        .and_then(|smart| smart.get("allowCrossProviderDiversity"))
        .and_then(Value::as_bool)
        // On by default: cross-checking is the point of Smart auto — verifier/
        // reviewer roles should land on a *different* provider than the main
        // model without any setup, and worker roles get best-of-breed instead of
        // same-provider anchoring. Single-provider pools are unaffected (the
        // inventory only ever contains connected models), and `/smart` can still
        // turn it off. Keep in lockstep with `snapshot_from_root` in the CLI.
        .unwrap_or(true);
    let verify_cross_provider = smart
        .and_then(|smart| smart.get("verifyCrossProvider"))
        .and_then(Value::as_bool)
        // On by default: the always-on deep-gate VERIFY leg should cross to a
        // different provider than the main model without any setup. Governs
        // ONLY the verify leg — decoupled from `allowCrossProviderDiversity`
        // (the worker-diversity flag) so one can be off while the other is on.
        // Keep in lockstep with `snapshot_from_root` in the CLI and
        // `smart_setting_defaults()`.
        .unwrap_or(true);
    let quota_fallback = smart
        .and_then(|smart| smart.get("quotaFallback"))
        .and_then(Value::as_bool)
        // On by default: a main-model quota exhaustion should auto-continue on an
        // equivalent different-provider model for that turn instead of killing
        // the turn. Keep in lockstep with `snapshot_from_root` in the CLI and
        // `smart_setting_defaults()`.
        .unwrap_or(true);
    let quota_wait_band_minutes = quota_wait_band_minutes_from_smart(smart);
    let provider_allowlist = provider_allowlist_from_smart(smart);
    let deep_tier_setting = deep_tier_models_from_smart(smart);
    let feedback_informed_auto = smart
        .and_then(|smart| smart.get("feedbackInformedAuto"))
        .and_then(Value::as_bool)
        // On by default: the outcome loop is how routing stays dynamic (learns who
        // performs the role) instead of frozen on the model-name/recency prior. It
        // is bounded + confidence-weighted (see `RouteOutcomeBucket`), and the user
        // can still disable it via `/smart`.
        .unwrap_or(true);
    // Dynamic by default: a spawn's complexity is the fused verdict of the
    // keyword tables and one bounded Fast-tier probe (`Probed`), so "easy"
    // and "large" are the model's reading of the task, not a word count. An
    // explicit `autoClassifier` still chooses; only its absence means probed.
    let auto_classifier = match smart.and_then(|smart| smart.get("autoClassifier")) {
        None => RouteAutoClassifierMode::Probed,
        value @ Some(_) => RouteAutoClassifierMode::from_settings_value(value),
    };
    let classifier_fallback = smart
        .and_then(|smart| smart.get("classifierFallback"))
        .and_then(Value::as_str)
        .and_then(runtime::ClassifierFallback::from_word)
        .unwrap_or_default();
    let fallback_candidate_limit = smart
        .and_then(|smart| smart.get("fallbackCandidateLimit"))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|&value| value > 0)
        .unwrap_or(DEFAULT_FALLBACK_CANDIDATE_LIMIT);
    let exploration = smart
        .and_then(|smart| smart.get("exploration"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let exploration_cadence = smart
        .and_then(|smart| smart.get("explorationCadence"))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|&value| value > 0)
        .unwrap_or(DEFAULT_EXPLORATION_CADENCE);
    let learned_specialty =
        LearnedSpecialtyMode::from_settings_value(smart.and_then(|smart| smart.get("learnedSpecialty")));
    let policy = SmartPolicy::from_settings_value(smart.and_then(|smart| smart.get("policy")));
    let exec_swap =
        SmartExecSwap::from_settings_value(smart.and_then(|smart| smart.get("execSwap")));
    let orchestration = HostOrchestration::from_settings_value(
        smart.and_then(|smart| smart.get("orchestration")),
    );
    let headroom_penalty_threshold = smart
        .and_then(|smart| smart.get("headroomPenaltyThreshold"))
        .and_then(Value::as_u64)
        // Clamp to 1..=100 (not `filter(>0)`): a 0 threshold would divide-by-zero
        // the graded penalty, and >100 is meaningless — pin both ends into range
        // rather than silently falling back to the default. Keep in lockstep with
        // `snapshot_from_root` in the CLI.
        .map(|value| value.clamp(1, 100))
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(DEFAULT_HEADROOM_PENALTY_THRESHOLD);
    let plan = PlanShadowSettings::from_settings_value(smart.and_then(|smart| smart.get("plan")));
    let mut subagents = BTreeMap::new();
    if let Some(router) = root.get("modelRouter").and_then(Value::as_object) {
        if let Some(subagent_object) = router.get("subagents").and_then(Value::as_object) {
            for (subagent, value) in subagent_object {
                let Some(profile) = SubagentProfileId::parse(subagent) else {
                    continue;
                };
                if let Ok(Some(override_rule)) = role_override_from_json(value) {
                    subagents.insert(profile.key().to_string(), override_rule);
                }
            }
        }
    }
    let mut roles = BTreeMap::new();
    if let Some(router) = root.get("modelRouter").and_then(Value::as_object) {
        if let Some(role_object) = router.get("roles").and_then(Value::as_object) {
            for (role, value) in role_object {
                let Some(route_role) = route_role_from_key(role) else {
                    continue;
                };
                if let Ok(Some(override_rule)) = role_override_from_json(value) {
                    roles.insert(role_key(route_role).to_string(), override_rule);
                }
            }
        }
    }
    Some(SmartRuntimeSettings {
        enabled,
        allow_cross_provider_diversity,
        verify_cross_provider,
        quota_fallback,
        quota_wait_band_minutes,
        provider_allowlist,
        deep_tier_models: deep_tier_setting.models,
        deep_tier_models_configured: deep_tier_setting.configured,
        feedback_informed_auto,
        auto_classifier,
        subagents,
        roles,
        fallback_candidate_limit,
        exploration,
        exploration_cadence,
        learned_specialty,
        headroom_penalty_threshold,
        policy,
        exec_swap,
        classifier_fallback,
        orchestration,
        plan,
    })
}

/// The settings a use's mode is read from — the global file merged with the
/// project's, as every other reader of these words sees them.
#[must_use]
pub fn merged_settings_root(cwd: &Path) -> Option<Value> {
    merged_settings_root_from(&runtime::ConfigLoader::default_for(cwd))
}

/// The same settings a use's mode is read from, off a loader already built —
/// the form the detached shadow needs, because by the time it runs the
/// environment that answers where settings live may have moved.
#[must_use]
pub(super) fn merged_settings_root_from(loader: &runtime::ConfigLoader) -> Option<Value> {
    let config = loader.load().ok()?;
    serde_json::from_str(&config.as_json().render()).ok()
}

/// The environment override for the anchor marker's TTL, for a harness or an
/// A/B that must not edit a settings file.
pub const CACHE_ANCHOR_TTL_ENV: &str = "ZO_CACHE_ANCHOR_TTL";

/// The conversation anchor marker's TTL policy for a turn under `cwd`:
/// [`CACHE_ANCHOR_TTL_ENV`] when set, else `cache.anchorTtl` from the merged
/// settings (`"5m"` / `"1h"`), else five minutes — r21's choice, kept as
/// the default until the ledger has judged the 1h anchor
/// (`runtime::mark_conversation_cache_breakpoints` docs, 2026-09-16).
#[must_use]
pub fn conversation_anchor_ttl_for(cwd: &Path) -> runtime::ConversationAnchorTtl {
    let from_env = std::env::var(CACHE_ANCHOR_TTL_ENV)
        .ok()
        .and_then(|label| runtime::ConversationAnchorTtl::from_label(&label));
    from_env.unwrap_or_else(|| conversation_anchor_ttl_from_root(merged_settings_root(cwd).as_ref()))
}

/// The pure half of [`conversation_anchor_ttl_for`]: the policy a merged
/// settings document declares under `cache.anchorTtl`, five minutes when it
/// declares none or spells one the ladder does not know.
#[must_use]
pub fn conversation_anchor_ttl_from_root(root: Option<&Value>) -> runtime::ConversationAnchorTtl {
    root.and_then(|root| root.get("cache"))
        .and_then(|cache| cache.get("anchorTtl"))
        .and_then(Value::as_str)
        .and_then(runtime::ConversationAnchorTtl::from_label)
        .unwrap_or_default()
}

/// The foreground Architect EXEC-swap policy from merged
/// [`runtime::ConfigLoader`] settings, so project and `--settings` overlays
/// participate. A loader failure keeps EXEC native.
#[must_use]
pub fn smart_exec_swap() -> SmartExecSwap {
    read_smart_runtime_settings().map_or(SmartExecSwap::Never, |settings| settings.exec_swap)
}

/// Ordered Architect PLAN/VERIFY pool from merged [`runtime::ConfigLoader`]
/// settings. A load failure keeps the built-in safety pool.
#[must_use]
pub fn smart_deep_tier_models() -> Vec<String> {
    read_smart_runtime_settings().map_or_else(
        runtime::default_deep_tier_models,
        |settings| settings.deep_tier_models,
    )
}

/// Ordered Architect PLAN/VERIFY pool and whether a non-empty merged setting
/// replaced the built-in default for `cwd`.
#[must_use]
pub fn smart_deep_tier_models_for(cwd: &Path) -> Option<DeepTierModelsSetting> {
    read_smart_runtime_settings_for(cwd).map(|settings| DeepTierModelsSetting {
        models: settings.deep_tier_models,
        configured: settings.deep_tier_models_configured,
    })
}

/// Resolve every Smart model choice a foreground runtime needs at turn entry.
///
/// The settings file is merged and parsed once, so quota, verify, PLAN, and
/// EXEC cannot observe different snapshots when an overlay changes. A load
/// failure preserves the pre-Smart behavior (no alternate clients or
/// Architect contract) while retaining the documented wait-band and deep-pool
/// defaults for the runtime slots that must always be refreshed.
#[must_use]
pub fn smart_turn_routing_for(
    cwd: &Path,
    main_model: &str,
    complexity: RouteTaskComplexity,
) -> SmartTurnRouting {
    smart_turn_routing_and_inventory_for(cwd, main_model, complexity).0
}

/// [`smart_turn_routing_for`] that also hands back the inventory it probed,
/// so a caller that needs the connected models for the same turn (the plan
/// scorer's shadow) reads the one probe instead of paying a second.
#[must_use]
pub fn smart_turn_routing_and_inventory_for(
    cwd: &Path,
    main_model: &str,
    complexity: RouteTaskComplexity,
) -> (SmartTurnRouting, runtime::ModelInventory) {
    let settings = read_smart_runtime_settings_for(cwd);
    let inventory = connected_model_inventory(main_model.trim());
    let routing = smart_turn_routing_with(settings, main_model, &inventory, complexity);
    (routing, inventory)
}

/// [`smart_turn_routing_for`] over an already-read settings snapshot and an
/// already-built inventory: one snapshot and one provider probe serve quota,
/// VERIFY, PLAN and EXEC alike, and the tests route through here with a fake
/// inventory instead of live credentials.
pub(super) fn smart_turn_routing_with(
    settings: Option<SmartRuntimeSettings>,
    main_model: &str,
    inventory: &runtime::ModelInventory,
    complexity: RouteTaskComplexity,
) -> SmartTurnRouting {
    let main_model = main_model.trim();
    let Some(settings) = settings else {
        return SmartTurnRouting {
            quota_fallback_model: None,
            refusal_fallback_model: None,
            classifier_fallback: runtime::ClassifierFallback::default(),
            quota_wait_band: std::time::Duration::from_secs(
                DEFAULT_QUOTA_WAIT_BAND_MINUTES.saturating_mul(60),
            ),
            deep_verify_model: None,
            deep_plan_model: None,
            deep_tier_only: false,
            deep_tier_models: effective_deep_tier_models(&[], false, main_model, inventory),
            exec_impl_model: None,
            exec_swap: SmartExecSwap::Never,
            // No readable settings: the host never spawns on its own, and
            // the shadow scorer stays quiet too.
            orchestration: HostOrchestration::ModelLed,
            plan: PlanShadowSettings {
                shadow: false,
                ..PlanShadowSettings::default()
            },
        };
    };

    let deep_tier_models = effective_deep_tier_models(
        &settings.deep_tier_models,
        settings.deep_tier_models_configured,
        main_model,
        inventory,
    );
    let deep_tier_only = settings.enabled && settings.policy == SmartPolicy::Architect;
    let main_is_deep = runtime::is_deep_tier_model(main_model, &deep_tier_models);
    let deep_verify_model = route_deep_verify_model(main_model, &settings, &deep_tier_models, inventory);
    let deep_plan_model = (deep_tier_only && !main_is_deep)
        .then(|| first_distinct_deep_model(main_model, &deep_tier_models))
        .flatten();
    let exec_impl_model = (deep_tier_only && main_is_deep)
        .then(|| route_exec_impl_model(main_model, &settings, &deep_tier_models, inventory, complexity))
        .flatten();
    SmartTurnRouting {
        quota_fallback_model: route_quota_fallback_model(main_model, &settings, inventory),
        refusal_fallback_model: route_refusal_fallback_model(main_model, &settings, inventory),
        classifier_fallback: settings.classifier_fallback,
        quota_wait_band: std::time::Duration::from_secs(
            settings.quota_wait_band_minutes.saturating_mul(60),
        ),
        deep_verify_model,
        deep_plan_model,
        deep_tier_only,
        deep_tier_models,
        exec_impl_model,
        exec_swap: settings.exec_swap,
        orchestration: settings.orchestration,
        plan: settings.plan,
    }
}

/// The Architect PLAN/VERIFY pool for this turn: the connected top tier
/// ([`runtime::dynamic_deep_tier_models`]) as it stands now. A person-authored
/// `smart.deepTierModels` narrows it — an entry names a model the pool may
/// keep — and never widens it: an entry that is not a connected top-tier model
/// is dropped (said once on stderr), and a list that keeps no model besides the
/// main one yields the whole dynamic pool rather than an empty checker bench.
/// The main model stays in the pool whenever the inventory places it in the top
/// tier, list or no list, so "is the main model deep-tier" is a fact the pool
/// cannot misreport. With no Deep model connected the pool is the catalog's
/// declared reserved set, alias-resolved.
pub(super) fn effective_deep_tier_models(
    configured: &[String],
    configured_flag: bool,
    main_model: &str,
    inventory: &runtime::ModelInventory,
) -> Vec<String> {
    let mut dynamic = runtime::dynamic_deep_tier_models(inventory);
    if dynamic.is_empty() {
        dynamic = runtime::default_deep_tier_models()
            .iter()
            .map(|alias| api::resolve_catalog_alias(alias))
            .collect();
    }
    if !configured_flag || configured.is_empty() {
        return dynamic;
    }
    let listed = |model: &str| {
        configured
            .iter()
            .any(|entry| runtime::deep_tier_model_matches(model, entry))
    };
    let dropped: Vec<&str> = configured
        .iter()
        .filter(|entry| !dynamic.iter().any(|model| runtime::deep_tier_model_matches(model, entry)))
        .map(String::as_str)
        .collect();
    if !dropped.is_empty() {
        notice_dropped_deep_tier_entries(&dropped, &dynamic);
    }
    let kept_by_list = dynamic.iter().filter(|model| listed(model)).count();
    if kept_by_list == 0 {
        return dynamic;
    }
    dynamic
        .into_iter()
        .filter(|model| same_smart_model(model, main_model) || listed(model))
        .collect()
}

/// Say once per distinct dropped set which `smart.deepTierModels` entries the
/// connected top tier does not contain, and what the pool is instead.
fn notice_dropped_deep_tier_entries(dropped: &[&str], pool: &[String]) {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};
    static NOTICED: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let key = dropped.join(", ");
    let mut seen = NOTICED
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if seen.insert(key.clone()) {
        eprintln!(
            "[zo:smart] deepTierModels: {key} — not in the connected top tier; planning and verifying with {}",
            pool.join(", ")
        );
    }
}

fn route_quota_fallback_model(
    main_model: &str,
    settings: &SmartRuntimeSettings,
    inventory: &runtime::ModelInventory,
) -> Option<String> {
    if main_model.is_empty() || !settings.enabled || !settings.quota_fallback {
        return None;
    }
    let main_provider = api::detect_provider_kind(main_model);
    let request = RouteRequest::for_target(
        RoutingTarget::RoleFallback(RouteRole::Default),
        RouteRole::Default,
        main_model,
    )
    .with_context(RoutePolicyContext {
        risk: RouteTaskRisk::Medium,
        complexity: RouteTaskComplexity::Medium,
        context_need: RouteContextNeed::LocalFiles,
        tool_need: RouteToolNeed::Write,
        verification_need: RouteVerificationNeed::Full,
        allow_cross_provider_diversity: true,
        provider_allowlist: settings.provider_allowlist.clone(),
        feedback: RouteFeedbackHint::disabled(),
        audit_notes: vec!["quota-fallback".to_string()],
        learned_specialty: LearnedSpecialtyHint::disabled(),
        policy: settings.policy,
        ..RoutePolicyContext::default()
    });
    route_model_fallback_candidates(
        &request,
        inventory,
        main_model,
        settings.fallback_candidate_limit,
    )
    .into_iter()
    .find(|candidate| api::detect_provider_kind(candidate) != main_provider)
}

/// The cross-provider model a doubly-refused turn is handed to (t-6269): the
/// first candidate in the main model's catalog `refusal_fallback` list that is
/// on ANOTHER provider AND connected (in the inventory). Same-provider
/// candidates (Fable/Sonnet/Haiku → Opus) are the runtime's own bound-client
/// override, not a client the host installs, so they are skipped here.
///
/// This is the refusal peer of [`route_quota_fallback_model`]: a different
/// selection (the catalog's explicit order, not the router's score) feeding the
/// SAME client role and the SAME runtime swap machinery. `None` when the lineup
/// names no cross-provider candidate or none is connected — the refusal then
/// surfaces (or takes the context-cleaning retry) exactly as before.
fn route_refusal_fallback_model(
    main_model: &str,
    settings: &SmartRuntimeSettings,
    inventory: &runtime::ModelInventory,
) -> Option<String> {
    if main_model.is_empty() || !settings.enabled {
        return None;
    }
    // Like the main-model fallback, this explicit catalog fallback is not
    // constrained by `provider_allowlist` (which shapes AUTO routing): the
    // list is already a curated per-lineup order, and the point of the escape
    // is to reach whatever provider is connected when the main one declines.
    let main_provider = api::detect_provider_kind(main_model);
    api::refusal_fallback_candidates(main_model)
        .into_iter()
        .find(|candidate| {
            api::detect_provider_kind(candidate) != main_provider
                && inventory.find(&api::resolve_catalog_alias(candidate)).is_some()
        })
}

fn route_deep_verify_model(
    main_model: &str,
    settings: &SmartRuntimeSettings,
    deep_tier_models: &[String],
    inventory: &runtime::ModelInventory,
) -> Option<String> {
    if main_model.is_empty() || !settings.enabled {
        return None;
    }
    if let Some(RoleOverride::Pin(model)) = settings.roles.get(role_key(RouteRole::Verifier)) {
        return (!model.trim().is_empty() && !same_smart_model(model, main_model))
            .then(|| model.clone());
    }

    let candidates = if settings.policy == SmartPolicy::Architect {
        // The connected top tier in rank order: the verifier is the best
        // other model available now, never the first name in a list.
        deep_tier_models.to_vec()
    } else {
        let mut request = RouteRequest::for_target(
            RoutingTarget::RoleFallback(RouteRole::Verifier),
            RouteRole::Verifier,
            main_model,
        );
        request.override_rule = settings.roles.get(role_key(RouteRole::Verifier)).cloned();
        request = request.with_context(RoutePolicyContext {
            risk: RouteTaskRisk::Low,
            complexity: RouteTaskComplexity::Large,
            context_need: RouteContextNeed::LocalFiles,
            tool_need: RouteToolNeed::ReadOnly,
            verification_need: RouteVerificationNeed::Focused,
            allow_cross_provider_diversity: settings.verify_cross_provider,
            provider_allowlist: settings.provider_allowlist.clone(),
            feedback: RouteFeedbackHint::disabled(),
            audit_notes: vec!["deep-gate-verify".to_string()],
            learned_specialty: LearnedSpecialtyHint::disabled(),
            policy: settings.policy,
            ..RoutePolicyContext::default()
        });
        let primary = route_model(&request, inventory).resolved_model;
        let mut candidates = vec![primary.clone()];
        candidates.extend(route_model_fallback_candidates(
            &request,
            inventory,
            &primary,
            settings.fallback_candidate_limit,
        ));
        candidates
    };
    preferred_verify_model(
        main_model,
        candidates.into_iter(),
        settings.verify_cross_provider,
    )
}

fn preferred_verify_model(
    main_model: &str,
    candidates: impl Iterator<Item = String>,
    allow_cross_provider: bool,
) -> Option<String> {
    let main_provider = api::detect_provider_kind(main_model);
    let candidates = candidates
        .filter(|candidate| !same_smart_generation(candidate, main_model))
        .fold(Vec::<String>::new(), |mut unique, candidate| {
            if !unique
                .iter()
                .any(|existing| same_smart_model(existing, &candidate))
            {
                unique.push(candidate);
            }
            unique
        });
    if allow_cross_provider {
        candidates
            .iter()
            .find(|candidate| api::detect_provider_kind(candidate) != main_provider)
            .cloned()
            .or_else(|| candidates.into_iter().next())
    } else {
        candidates
            .into_iter()
            .find(|candidate| api::detect_provider_kind(candidate) == main_provider)
    }
}

fn first_distinct_deep_model(main_model: &str, deep_tier_models: &[String]) -> Option<String> {
    deep_tier_models
        .iter()
        .find(|candidate| !same_smart_generation(candidate, main_model))
        .cloned()
}

/// The same model, or another release of the same generation (a session on
/// `claude-fable-5` beside the catalog's `claude-fable-5-1`): neither is the
/// other's checker or implementer.
fn same_smart_generation(left: &str, right: &str) -> bool {
    same_smart_model(left, right)
        || runtime::deep_tier_model_matches(left, right)
        || runtime::deep_tier_model_matches(right, left)
}

/// Whether [`smart_turn_routing_for`] would arm an exec contract for this
/// main model — the SAME condition it uses (`Architect` policy, deep-tier
/// main, a routable implementer), asked ahead of the routing so the probe
/// gate can know whether an intent verdict will have a reader.
#[must_use]
pub(super) fn exec_impl_model_armed(main_model: &str, settings: &SmartRuntimeSettings) -> bool {
    let main_model = main_model.trim();
    let deep_tier_only = settings.enabled && settings.policy == SmartPolicy::Architect;
    if !deep_tier_only {
        return false;
    }
    let inventory = connected_model_inventory(main_model);
    let pool = effective_deep_tier_models(
        &settings.deep_tier_models,
        settings.deep_tier_models_configured,
        main_model,
        &inventory,
    );
    runtime::is_deep_tier_model(main_model, &pool)
        && route_exec_impl_model(main_model, settings, &pool, &inventory, RouteTaskComplexity::Medium).is_some()
}

/// The Architect EXEC implementer for this turn, walked by the turn's
/// complexity through the provider ladder (easy → the cheapest current
/// model, large → the second band); never the top band.
fn route_exec_impl_model(
    main_model: &str,
    settings: &SmartRuntimeSettings,
    deep_tier_models: &[String],
    inventory: &runtime::ModelInventory,
    complexity: RouteTaskComplexity,
) -> Option<String> {
    // An implementation lane like a spawn's: the parent's provider only (one
    // credential, one tool surface) and never the top band.
    let mut request = RouteRequest::for_target(
        RoutingTarget::WorkflowPhase { phase_id: "architect-exec-impl".to_string(), subagent: None },
        RouteRole::Coding,
        main_model,
    );
    request.override_rule = settings.roles.get(role_key(RouteRole::Coding)).cloned();
    request = request.with_context(RoutePolicyContext {
        risk: RouteTaskRisk::Medium,
        complexity,
        context_need: RouteContextNeed::MultiFile,
        tool_need: RouteToolNeed::Write,
        verification_need: RouteVerificationNeed::Focused,
        allow_cross_provider_diversity: settings.allow_cross_provider_diversity,
        provider_allowlist: settings.provider_allowlist.clone(),
        feedback: RouteFeedbackHint::disabled(),
        audit_notes: vec!["architect-exec-impl".to_string()],
        learned_specialty: LearnedSpecialtyHint::disabled(),
        policy: settings.policy,
        ..RoutePolicyContext::default()
    });
    if let Some(RoleOverride::Pin(model)) = request.override_rule.as_ref() {
        return (!model.is_empty()
            && !same_smart_model(model, main_model)
            && !runtime::is_deep_tier_model(model, deep_tier_models))
        .then(|| model.clone());
    }
    let primary = route_model(&request, inventory).resolved_model;
    let usable = |candidate: &String| {
        !same_smart_generation(candidate, main_model)
            && !runtime::is_deep_tier_model(candidate, deep_tier_models)
    };
    if usable(&primary) {
        return Some(primary);
    }
    route_model_fallback_candidates(
        &request,
        inventory,
        &primary,
        settings.fallback_candidate_limit,
    )
    .into_iter()
    .find(usable)
}

fn same_smart_model(left: &str, right: &str) -> bool {
    api::resolve_catalog_alias(left).eq_ignore_ascii_case(&api::resolve_catalog_alias(right))
}

/// One settings snapshot for the implementation gate. Only a person-authored
/// deep-tier list grants an exception; the shipped default is not a pin.
pub(crate) fn live_agent_model_policy() -> (SmartPolicy, Vec<String>) {
    read_smart_runtime_settings().map_or((SmartPolicy::Classic, Vec::new()), |settings| {
        (settings.policy, if settings.deep_tier_models_configured { settings.deep_tier_models } else { Vec::new() })
    })
}

/// Parse `smart.providerAllowlist` — a JSON array of provider names. Blank
/// entries are dropped; a missing/non-array value means "no restriction".
/// Keep in lockstep with `snapshot_from_root` in the CLI.
/// Parse `smart.quotaWaitBandMinutes` — minutes-to-reset within which the turn
/// loop holds on the main model instead of falling back. Absent → the default
/// band; `0` disables. Keep in lockstep with `snapshot_from_root` in the CLI.
fn quota_wait_band_minutes_from_smart(smart: Option<&serde_json::Map<String, Value>>) -> u64 {
    smart
        .and_then(|smart| smart.get("quotaWaitBandMinutes"))
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_QUOTA_WAIT_BAND_MINUTES)
}

fn deep_tier_models_from_smart(
    smart: Option<&serde_json::Map<String, Value>>,
) -> DeepTierModelsSetting {
    let configured = smart
        .and_then(|smart| smart.get("deepTierModels"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|model| !model.is_empty())
                // Catalog-first: a configured `openai-latest`/`google-latest`
                // must name the same canonical model whether or not that
                // provider is currently connected.
                .map(api::resolve_catalog_alias)
                .collect()
        })
        .filter(|models: &Vec<String>| !models.is_empty());
    configured.map_or_else(
        || DeepTierModelsSetting {
            models: runtime::default_deep_tier_models(),
            configured: false,
        },
        |models| DeepTierModelsSetting {
            models,
            configured: true,
        },
    )
}

pub(super) fn provider_allowlist_from_smart(
    smart: Option<&serde_json::Map<String, Value>>,
) -> Vec<String> {
    smart
        .and_then(|smart| smart.get("providerAllowlist"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn role_override_from_json(value: &Value) -> Result<Option<RoleOverride>, ()> {
    let object = value.as_object().ok_or(())?;
    match object.get("mode").and_then(Value::as_str).ok_or(())? {
        // Deletion tombstone: the CLI writes `{"mode":"deleted"}` into the
        // primary canonical root to durably mask a same-key override that lives
        // only in a lower root. The router must read it as "no override" (the
        // same as `auto`/absent), not as a parse error.
        "auto" | "deleted" => Ok(None),
        "pinned" => {
            let model = object.get("model").and_then(Value::as_str).ok_or(())?.trim();
            if model.is_empty() {
                return Err(());
            }
            Ok(Some(RoleOverride::Pin(model.to_string())))
        }
        "manualPreferred" => {
            let selector = object.get("selector").and_then(Value::as_object).ok_or(())?;
            let provider = required_selector_string(selector, "provider").ok_or(())?;
            let family = required_selector_string(selector, "family").ok_or(())?;
            let class = required_selector_string(selector, "class").ok_or(())?;
            let freshness = match required_selector_string(selector, "freshness").ok_or(())?.as_str() {
                "latest" => FreshnessPolicy::Latest,
                "latestStable" => FreshnessPolicy::LatestStable,
                _ => return Err(()),
            };
            Ok(Some(RoleOverride::Family(
                RoleSelector::new()
                    .provider(provider)
                    .family(family)
                    .class(class)
                    .freshness(freshness),
            )))
        }
        _ => Err(()),
    }
}

fn required_selector_string(
    selector: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<String> {
    let value = selector.get(key)?.as_str()?.trim();
    (!value.is_empty()).then_some(value.to_string())
}

// Outcome-feedback lookups live on `apply::SmartRouteContext::feedback_for`,
// which serves them from a batch-loaded summary instead of re-parsing the
// outcome log per agent. The keying contract (resolved subagent type,
// `subagent:{type}`) is documented there.
