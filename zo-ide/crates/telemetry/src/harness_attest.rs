//! Harness attestation ledger — proof that a harness feature actually RAN.
//!
//! Every routing / reasoning / context feature in zo is deliberately
//! fail-open: a routing probe that 400s, a reminder whose gate never opens, a
//! reasoning replay with nothing to replay — each degrades to "the plain path"
//! instead of surfacing an error. That is the right RUNTIME behavior and the
//! wrong OBSERVABILITY behavior, because it makes a dead feature
//! indistinguishable from an idle one. Real regressions hid in exactly that
//! gap for weeks at a time: the routing probe failed 100% of the time on a
//! wire error (see `smart_router::probe_exec`'s own post-mortem), and the
//! design-guidance reminder was structurally unreachable under a pinned
//! effort — both silent, both fail-open, both invisible until someone thought
//! to grep for a string that turned out to have zero occurrences.
//!
//! This ledger closes that gap with the cheapest mechanism that can work:
//! every fail-open branch bumps a named counter, and [`HarnessFeature::all`]
//! turns a feature with ZERO firings into a printed row rather than an absent
//! one. "Never fired" becomes an observation instead of the absence of one.
//!
//! # Why a bail-out has two kinds
//!
//! A zero-firing count on its own cannot tell a defect from normal operation:
//! a probe that never fires because the provider rejects every call and a
//! reminder that never fires because no design turn arrived look identical
//! from a single counter. So a bail-out is recorded as one of two KINDS, and
//! the distinction is the diagnosis:
//!
//! - [`attest_failed`] — the feature tried and something went wrong (a
//!   timeout, a wire error, an unparseable reply, an unreadable setting). Zero
//!   firings against a nonzero failure count is a live defect.
//! - [`attest_declined`] — a gate deliberately chose not to run it (the cost
//!   gate said the task was too small, the user switched the classifier off,
//!   the turn's intent was not `Design`). Zero firings against declines only
//!   is normal operation, and the reasons say which gate is holding.
//!
//! Getting this wrong in either direction is what makes a diagnostic table
//! worthless — cry wolf on ordinary declines and the reader learns to ignore
//! the table; fold real failures in with declines and the defect hides again.
//!
//! Deliberate non-goals: no timings, no payloads, no per-call detail. Those
//! already live in `ZO_ROUTE_DEBUG` and the route-outcome JSONL. This is a
//! counter table, so its footprint is bounded by the code's own cardinality —
//! which is exactly why a reason is a `&'static str` and never a formatted
//! string. A caller that formats reasons would grow this table without bound;
//! the type forbids it.
//!
//! Purity seam (mirrors `runtime::model_router::learned`): the ledger itself
//! is a plain value with no clock, no I/O, and no globals, so it can be tested
//! in full isolation. The process-wide instance is a thin wrapper on top —
//! necessary because the recording sites (the Responses wire in `api`, the
//! probe executor in `tools`, the turn controller in `zo-cli`) sit on pure
//! function paths that carry no shared context to thread a handle through.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// A harness feature whose firing is worth proving.
///
/// Every variant here must be a feature that can silently NOT run — a
/// mandatory code path needs no attestation. Adding a variant is how a new
/// fail-open branch becomes observable; [`Self::all`] is what makes its
/// zero-count visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HarnessFeature {
    /// The Fast-tier routing probe (`smart.autoClassifier: "probed"`) that
    /// fuses a model self-assessment over the deterministic classifier.
    RoutingProbe,
    /// The typed routing judgment: in record-only mode it runs beside the chat
    /// probe; in actual-use mode it runs first and validated answers feed the
    /// same conservative fusion boundary. Declined `off` and `no_key`; failed
    /// under the adapter's token.
    DecisionShadow,
    /// The memory rerank in shadow (`smart.rerankShadow: "shadow"`): every
    /// recall's notes put to a System One judgment off the turn's thread, the
    /// order it would have proposed written beside recall's, and nothing the
    /// turn reads changed. Declined `off` and `no_key`; failed under the
    /// adapter's token.
    RerankShadow,
    /// The skill search (`smart.skillSearch`): every installed skill put to a
    /// System One judgment against the task a turn describes, so the best of
    /// them come back as a tool result instead of the whole catalog riding
    /// the system prompt. Declined `off` and `no_key`; failed under the
    /// adapter's token.
    SkillSearch,
    /// Per-tool-call reasoning replay on the OpenAI Responses wire — the
    /// items resent ahead of a function call so the model keeps its own chain
    /// of thought across a tool round-trip.
    ReasoningReplayCall,
    /// Turn-boundary reasoning replay on the same wire: the items belonging to
    /// a response that ended in TEXT rather than a tool call. Without this a
    /// multi-turn conversation re-derives its reasoning every turn — the
    /// "retained reasoning" setting that tripled OpenAI's own ARC-AGI-3 score.
    ReasoningReplayTurnFinal,
    /// The `[zo:design-guidance]` reminder injected for a turn whose resolved
    /// intent is `Design`.
    DesignGuidance,
    /// Information-topology `sees` edges (the Fugu access-list gap): a spawn
    /// or steer that injected a finished agent's deliverable into another
    /// agent's context engine-side, instead of the parent re-narrating it.
    InformationTopology,
    /// The workflow engine's `{seen}` relay: prior-round results carried into
    /// the next round of a repeating phase, so round N+1 does not re-derive what
    /// round N already established. The relay is what makes a repeat a
    /// CONTINUATION rather than a fresh attempt at the same instruction.
    WorkflowRelay,
    /// The empty-response retry ladder: a turn whose previous attempt returned
    /// no visible output goes back out at a LOWER reasoning rung, so the retry
    /// can end somewhere the first attempt could not. Attested because a retry
    /// that works leaves no other durable trace — its whole success condition
    /// is that the user never sees the empty turn.
    EmptyRetryDeescalation,
    /// A deep-gate `[auto:RETRY]` chain converting: `fired` = a turn whose
    /// verifier rejected at least once ended Accepted after the retry;
    /// `failed(gave_up)` = the retry budget was spent and the loop still gave
    /// up. Measured live at ~$4 median per retry (84 turns), so the
    /// fired:failed ratio is the evidence that tunes max attempts — without
    /// it every retry-budget change is blind.
    DeepRetryConversion,
    /// Mid-generation steering re-issue ("gen-abort"): a correction typed while
    /// the model was still generating abandoned the in-flight call and re-issued
    /// it with the steer folded in, rather than letting the steer wait behind a
    /// stale tool batch.
    ///
    /// Attested because a working re-issue leaves NO durable trace: the
    /// abandoned call settles nothing at all — no assistant message, no
    /// iteration record — so the transcript of a steer delivered early and the
    /// transcript of a steer that silently waited for the tool-result boundary
    /// are the same transcript. The race is also fail-open by construction (a
    /// turn being torn down, a call that already emitted `tool_use`, and an
    /// unreadable steering queue all fall through to the old boundary fold), so
    /// a regression that stops abandoning is invisible from the outside.
    SteeringReissue,
    /// Esc-once tool cancellation: a dispatch the user stopped mid-flight
    /// settled as cancelled and the turn carried on.
    ///
    /// Attested because the feature is at its most invisible when it works —
    /// what the user sees is a turn that kept going, which is exactly what they
    /// see when nothing was cancelled at all. The cancel is armed per dispatch
    /// off a signal epoch, so it fails open in both directions (a keypress
    /// outside a dispatch is inert, and a tool that finishes in the same poll
    /// wins the race deliberately); neither leaves a trace of its own.
    ToolCancelSettled,
    /// The `[zo:verified-state]` observation injected at turn start: the
    /// check-shaped bash commands the runtime recorded exiting 0 in this
    /// session, plus which files have been edited since each one.
    ///
    /// Attested because the injection is CONDITIONAL and therefore silently
    /// skippable: a session that never ran a check produces no block at all
    /// (deliberately — byte neutrality), which is byte-for-byte identical to a
    /// session whose ledger never got bound, never got persisted, or lost its
    /// sidecar across a stage boundary. Without a counter the two are the same
    /// observation, and the failure mode being guarded here — a ledger that is
    /// empty at exactly the moment it should have spoken — is precisely the
    /// invisible one.
    VerifiedStateObservation,
    /// Session-ledger green checks joined into a deep-gate VERIFY leg's
    /// objective section, beside this attempt's own post-edit greens.
    ///
    /// Counted apart from [`Self::VerifiedStateObservation`] because they
    /// measure different things and can diverge completely: the observation
    /// fires whenever the session has ANY recorded check, while the leg join
    /// fires only when the ledger knows a still-fresh check the attempt's own
    /// summary does not. Since the attempt's edits invalidate every older
    /// check by construction, that surplus is narrow by design — which is
    /// exactly why it needs its own counter rather than an assumption. Folded
    /// into the observation's row, a busy observation count would have made a
    /// join that never once contributed look perfectly healthy.
    VerifiedStateVerifyLeg,
}

impl HarnessFeature {
    /// Stable lowercase key, for log lines and any future persisted schema.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::RoutingProbe => "routing_probe",
            Self::DecisionShadow => "decision_shadow",
            Self::RerankShadow => "rerank_shadow",
            Self::SkillSearch => "skill_search",
            Self::ReasoningReplayCall => "reasoning_replay_call",
            Self::ReasoningReplayTurnFinal => "reasoning_replay_turn_final",
            Self::DesignGuidance => "design_guidance",
            Self::InformationTopology => "info_topology",
            Self::WorkflowRelay => "workflow_relay",
            Self::EmptyRetryDeescalation => "empty_retry_deescalation",
            Self::DeepRetryConversion => "deep_retry_conversion",
            Self::SteeringReissue => "steering_reissue",
            Self::ToolCancelSettled => "tool_cancel_settled",
            Self::VerifiedStateObservation => "verified_state_observation",
            Self::VerifiedStateVerifyLeg => "verified_state_verify_leg",
        }
    }

    /// Short human label for the `/smart doctor` table.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::RoutingProbe => "routing probe",
            Self::DecisionShadow => "TypeSafe routing judgment",
            Self::RerankShadow => "rerank shadow (memory judgment)",
            Self::SkillSearch => "skill search (skill judgment)",
            Self::ReasoningReplayCall => "reasoning replay (tool call)",
            Self::ReasoningReplayTurnFinal => "reasoning replay (turn boundary)",
            Self::DesignGuidance => "design guidance reminder",
            Self::InformationTopology => "information topology (sees edges)",
            Self::WorkflowRelay => "workflow {seen} relay",
            Self::EmptyRetryDeescalation => "empty-response retry de-escalation",
            Self::DeepRetryConversion => "deep retry conversion",
            Self::SteeringReissue => "mid-generation steering re-issue",
            Self::ToolCancelSettled => "Esc tool cancel (settled)",
            Self::VerifiedStateObservation => "verified-state observation",
            Self::VerifiedStateVerifyLeg => "verified-state checks in VERIFY leg",
        }
    }

    /// The condition under which this feature is EXPECTED to fire, phrased so
    /// a reader can tell a dead feature from a legitimately idle one.
    ///
    /// Without this, a zero count on a never-entered path is ambiguous and
    /// therefore useless: the reasoning-replay rows are structurally silent on
    /// an Anthropic-only session (that wire has no replay to do), and
    /// reporting it as a defect would train the reader to ignore the whole
    /// table — the failure mode this module exists to prevent.
    #[must_use]
    pub const fn precondition(self) -> &'static str {
        match self {
            Self::RoutingProbe => "requires smart.autoClassifier = \"probed\"",
            Self::DecisionShadow => {
                "requires a routing decision, smart.decisionShadow = \"shadow\" or \"on\", and TYPESAFE_API_KEY"
            }
            Self::RerankShadow => {
                "requires a recall that returned notes, smart.rerankShadow = \"shadow\", and TYPESAFE_API_KEY"
            }
            Self::SkillSearch => {
                "requires a skill_search call, an installed skill, smart.skillSearch on, and TYPESAFE_API_KEY"
            }
            Self::ReasoningReplayCall | Self::ReasoningReplayTurnFinal => {
                "OpenAI Responses wire only; silent on an Anthropic-only session"
            }
            Self::DesignGuidance => "requires a turn whose resolved intent is Design",
            Self::InformationTopology => {
                "requires an Agent spawn with sees:[...] or a SendMessage with attach_results"
            }
            Self::WorkflowRelay => "requires a workflow repeat phase reaching round 2",
            Self::EmptyRetryDeescalation => {
                "OpenAI Responses wire only; requires a turn that already came back empty"
            }
            Self::DeepRetryConversion => {
                "requires a deep-gate turn whose verifier rejected an attempt (an [auto:RETRY] follow-up ran)"
            }
            Self::SteeringReissue => {
                "requires steering typed while a call is still generating, before it emits any tool_use"
            }
            Self::ToolCancelSettled => "requires Esc pressed while a tool dispatch is in flight",
            Self::VerifiedStateObservation => {
                "requires a turn start after this session recorded a check-shaped bash command exiting 0"
            }
            Self::VerifiedStateVerifyLeg => {
                "requires a deep-gate VERIFY leg while the ledger holds a still-fresh check the attempt did not itself report"
            }
        }
    }

    /// Every attested feature, in display order. The reason this exists: a
    /// snapshot built only from recorded entries can never show a feature that
    /// recorded nothing, which is precisely the case worth seeing.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::RoutingProbe,
            Self::DecisionShadow,
            Self::RerankShadow,
            Self::SkillSearch,
            Self::ReasoningReplayCall,
            Self::ReasoningReplayTurnFinal,
            Self::DesignGuidance,
            Self::InformationTopology,
            Self::WorkflowRelay,
            Self::EmptyRetryDeescalation,
            Self::DeepRetryConversion,
            Self::SteeringReissue,
            Self::ToolCancelSettled,
            Self::VerifiedStateObservation,
            Self::VerifiedStateVerifyLeg,
        ]
    }

    /// The inverse of [`Self::key`], case-insensitively. This is what lets an
    /// ablation be named from outside the process; nothing else may invent a
    /// feature name, so the key set stays the single vocabulary.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|feature| feature.key().eq_ignore_ascii_case(key.trim()))
    }

    /// Whether an experiment may hold this feature out (`ZO_ABLATE`).
    ///
    /// A harness-initiated policy is ablatable: the harness chose to run it,
    /// so an experiment can meaningfully measure the arm where it chose not
    /// to. A USER-initiated interaction is not — suppressing the steering
    /// re-issue or the Esc tool cancel would mean ignoring an explicit user
    /// action mid-session, so there is no counterfactual arm to measure, only
    /// a broken one. Excluding these here (rather than gating their call
    /// sites on `attest_ablated`) makes the leak state unrepresentable: a
    /// feature that can never enter an [`AblationSet`] can never fire inside
    /// one.
    #[must_use]
    pub const fn ablatable(self) -> bool {
        !matches!(self, Self::SteeringReissue | Self::ToolCancelSettled)
    }
}

/// Environment variable naming the features suppressed in this process, as a
/// comma-separated list of [`HarnessFeature::key`] values (or `all`).
pub const ABLATION_ENV: &str = "ZO_ABLATE";

/// The decline reason recorded when an ablation suppresses a feature. It is a
/// decline rather than a failure because nothing went wrong — an experiment
/// asked for the feature to stay out of the way.
pub const ABLATED_REASON: &str = "ablated";

/// A malformed ablation spec. Deliberately a hard error rather than a skipped
/// token: an experiment whose treatment silently failed to apply produces a
/// confident measurement of nothing, which is worse than no measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AblationSpecError {
    /// The token that named no feature, or named one that cannot be held out.
    pub token: String,
    /// True when the token names a real feature that is not
    /// [`HarnessFeature::ablatable`] — a distinct message, because "you
    /// misspelled it" and "this cannot be an experiment arm" call for
    /// different fixes.
    pub not_ablatable: bool,
}

impl std::fmt::Display for AblationSpecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.not_ablatable {
            return write!(
                formatter,
                "{ABLATION_ENV}: '{}' is user-initiated and cannot be held out — suppressing it \
                 would mean ignoring an explicit user action, so there is no control arm to measure",
                self.token
            );
        }
        let known = HarnessFeature::all()
            .iter()
            .filter(|feature| feature.ablatable())
            .map(|feature| feature.key())
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            formatter,
            "{ABLATION_ENV}: '{}' is not a harness feature (expected one of: {known}, or `all`)",
            self.token
        )
    }
}

impl std::error::Error for AblationSpecError {}

/// The set of features an experiment has switched off.
///
/// Ablation is how a harness feature's VALUE becomes measurable. The
/// attestation counters above prove a feature ran; they cannot say whether
/// running it helped, because there is no arm of the experiment where it did
/// not run. This type is that other arm — the same task, the same model, one
/// feature held out — so a difference in outcome is attributable to the
/// feature rather than to belief.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AblationSet {
    features: std::collections::BTreeSet<HarnessFeature>,
}

impl AblationSet {
    /// Nothing suppressed — the ordinary production arm.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Every ablatable feature suppressed — the floor an experiment measures
    /// the full harness against. User-initiated features
    /// ([`HarnessFeature::ablatable`] is false) stay out: they are not part of
    /// any experiment arm, so `all` holding them out would make a user's Esc
    /// or mid-turn correction silently inert inside the arm.
    #[must_use]
    pub fn all() -> Self {
        Self {
            features: HarnessFeature::all()
                .iter()
                .copied()
                .filter(|feature| feature.ablatable())
                .collect(),
        }
    }

    /// Parse a comma-separated spec of feature keys. Whitespace around a token
    /// is ignored, empty tokens are skipped (so a trailing comma is harmless),
    /// `all` expands to every feature, and an unknown token is an error.
    pub fn parse(spec: &str) -> Result<Self, AblationSpecError> {
        let mut features = std::collections::BTreeSet::new();
        for token in spec.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            if token.eq_ignore_ascii_case("all") {
                features.extend(
                    HarnessFeature::all()
                        .iter()
                        .copied()
                        .filter(|feature| feature.ablatable()),
                );
                continue;
            }
            let Some(feature) = HarnessFeature::from_key(token) else {
                return Err(AblationSpecError {
                    token: token.to_string(),
                    not_ablatable: false,
                });
            };
            // Naming a non-ablatable feature explicitly is a hard error, not a
            // skip: the site never consults the gate, so accepting the token
            // would run the treatment arm while reporting a holdout.
            if !feature.ablatable() {
                return Err(AblationSpecError {
                    token: token.to_string(),
                    not_ablatable: true,
                });
            }
            features.insert(feature);
        }
        Ok(Self { features })
    }

    #[must_use]
    pub fn contains(&self, feature: HarnessFeature) -> bool {
        self.features.contains(&feature)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// The suppressed features' keys, in display order — what a run's own
    /// report prints so a result carries the arm it was measured in.
    #[must_use]
    pub fn keys(&self) -> Vec<&'static str> {
        HarnessFeature::all()
            .iter()
            .copied()
            .filter(|feature| self.contains(*feature))
            .map(HarnessFeature::key)
            .collect()
    }
}

/// What a feature's counters say about it. Ordered by how much it should worry
/// a reader, so a diagnostic surface can sort or filter on it directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FeatureHealth {
    /// Fired at least once — the feature demonstrably works.
    Alive,
    /// Never fired because an ablation held it out. The control arm of an
    /// experiment, and the only zero that is a RESULT rather than a finding.
    Ablated,
    /// Never entered: no firing, no failure, no decline. The path is unwired,
    /// or an earlier gate closed before this feature was ever consulted. Read
    /// it against [`HarnessFeature::precondition`] before calling it a bug.
    Silent,
    /// Never fired, and every bail-out was a deliberate gate decision. Normal
    /// operation; the decline reasons name the gate that is holding.
    GatedOff,
    /// Never fired, and it FAILED at least once. This is the live fail-open
    /// defect signature — the shape the 100%-failing routing probe had for
    /// weeks while emitting nothing at all.
    Failing,
    /// Ablated and fired anyway: some path reached the feature without passing
    /// its gate. The measurement taken in this run is INVALID — the control
    /// arm still received the treatment — so this outranks every other state.
    AblationLeak,
}

/// One feature's counters: firings, plus per-reason tallies of the two kinds
/// of bail-out. Reasons are `&'static str` by construction — see the module
/// doc for why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureAttestation {
    pub fired: u64,
    /// Firings that happened while this feature was configured as ablated.
    ///
    /// Counted at the firing itself rather than inferred from a recorded
    /// decline, because the case worth catching is a call site that never
    /// consults the gate at all: such a site records no decline, so inferring
    /// the holdout from decline history would read the one bypass that
    /// matters as an ordinary healthy firing.
    pub leaked: u64,
    /// Times the feature tried and something went wrong, by reason.
    pub failed: BTreeMap<&'static str, u64>,
    /// Times a gate deliberately chose not to run it, by reason.
    pub declined: BTreeMap<&'static str, u64>,
}

impl FeatureAttestation {
    /// Times this feature tried and failed, across every reason.
    #[must_use]
    pub fn failed_total(&self) -> u64 {
        self.failed.values().sum()
    }

    /// Times a gate declined to run this feature, across every reason.
    #[must_use]
    pub fn declined_total(&self) -> u64 {
        self.declined.values().sum()
    }

    /// True when an ablation suppressed this feature at least once.
    #[must_use]
    pub fn was_ablated(&self) -> bool {
        self.declined.contains_key(ABLATED_REASON)
    }

    /// This feature's health — the single read a diagnostic surface needs.
    ///
    /// A real failure is checked BEFORE the ablation, so switching a feature
    /// off can never bury a defect on a path that ran anyway: an ablated
    /// feature that both failed and never fired still reads as `Failing`,
    /// which is the finding worth surfacing.
    ///
    /// Only a FIRING promotes to [`FeatureHealth::AblationLeak`], never a
    /// failure. Some attested failures are recorded UPSTREAM of the gate by
    /// design — `smart_router::turn` attests `settings_unavailable` for the
    /// routing probe before `probe_exec` is ever entered — so treating any
    /// failure under a holdout as a bypass would condemn sound control runs
    /// on evidence that proves nothing about the gate.
    #[must_use]
    pub fn health(&self) -> FeatureHealth {
        if self.leaked > 0 {
            FeatureHealth::AblationLeak
        } else if self.fired > 0 {
            FeatureHealth::Alive
        } else if !self.failed.is_empty() {
            FeatureHealth::Failing
        } else if self.was_ablated() {
            FeatureHealth::Ablated
        } else if !self.declined.is_empty() {
            FeatureHealth::GatedOff
        } else {
            FeatureHealth::Silent
        }
    }

    /// Failure reasons, most frequent first (name breaks ties for stability) —
    /// the order a diagnostic surface wants, since the top one is the one to
    /// fix.
    #[must_use]
    pub fn failures_by_frequency(&self) -> Vec<(&'static str, u64)> {
        rank_reasons(&self.failed)
    }

    /// Decline reasons, most frequent first — the top one is the gate that is
    /// actually holding this feature back.
    #[must_use]
    pub fn declines_by_frequency(&self) -> Vec<(&'static str, u64)> {
        rank_reasons(&self.declined)
    }
}

fn rank_reasons(reasons: &BTreeMap<&'static str, u64>) -> Vec<(&'static str, u64)> {
    let mut ranked: Vec<(&'static str, u64)> =
        reasons.iter().map(|(reason, count)| (*reason, *count)).collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    ranked
}

/// A pure counter table over [`HarnessFeature`]. No clock, no I/O, no global
/// reads — construct one, record into it, snapshot it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessAttestLedger {
    features: BTreeMap<HarnessFeature, FeatureAttestation>,
    /// The holdout this ledger is counting under. The ledger owns it so that
    /// a firing is classified against the CONFIGURED ablation rather than
    /// against whatever declines happened to be recorded first — a call site
    /// that skips the gate entirely records no decline, and that is precisely
    /// the bypass leak detection exists to catch.
    ablation: AblationSet,
}

impl HarnessAttestLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A ledger counting under `ablation`.
    #[must_use]
    pub fn with_ablation(ablation: AblationSet) -> Self {
        Self {
            features: BTreeMap::new(),
            ablation,
        }
    }

    /// The holdout this ledger classifies firings against.
    #[must_use]
    pub fn ablation(&self) -> &AblationSet {
        &self.ablation
    }

    /// Record one successful firing, marking it a leak when the feature was
    /// supposed to be held out.
    pub fn record_fired(&mut self, feature: HarnessFeature) {
        let ablated = self.ablation.contains(feature);
        let attestation = self.features.entry(feature).or_default();
        attestation.fired += 1;
        if ablated {
            attestation.leaked += 1;
        }
    }

    /// Record one bail-out where the feature tried and something went wrong.
    pub fn record_failed(&mut self, feature: HarnessFeature, reason: &'static str) {
        *self
            .features
            .entry(feature)
            .or_default()
            .failed
            .entry(reason)
            .or_default() += 1;
    }

    /// Record one bail-out where a gate deliberately chose not to run it.
    pub fn record_declined(&mut self, feature: HarnessFeature, reason: &'static str) {
        *self
            .features
            .entry(feature)
            .or_default()
            .declined
            .entry(reason)
            .or_default() += 1;
    }

    /// Consult this ledger's ablation for `feature`, recording the decline
    /// when it is suppressed, and report whether the caller must skip it.
    ///
    /// Query and record are one operation on purpose. A call site that had to
    /// ask and then remember to record could answer the first correctly and
    /// forget the second, and a suppressed feature that recorded nothing is
    /// indistinguishable from one that was never reached — exactly the
    /// ambiguity this module exists to remove.
    pub fn record_ablation(&mut self, feature: HarnessFeature) -> bool {
        if !self.ablation.contains(feature) {
            return false;
        }
        self.record_declined(feature, ABLATED_REASON);
        true
    }

    /// This feature's counters, or an all-zero [`FeatureAttestation`] when it
    /// recorded nothing — a lookup can never fail, so a caller never has to
    /// decide what a missing entry means.
    #[must_use]
    pub fn attestation(&self, feature: HarnessFeature) -> FeatureAttestation {
        self.features.get(&feature).cloned().unwrap_or_default()
    }

    /// Every feature in [`HarnessFeature::all`] order, including the ones that
    /// recorded nothing. This is the whole point of the module: the rows with
    /// zeros are the finding.
    #[must_use]
    pub fn rows(&self) -> Vec<(HarnessFeature, FeatureAttestation)> {
        HarnessFeature::all()
            .iter()
            .copied()
            .map(|feature| (feature, self.attestation(feature)))
            .collect()
    }

    /// Features whose counters read as [`FeatureHealth::Failing`] — the rows a
    /// caller should surface first, or escalate on.
    #[must_use]
    pub fn failing_features(&self) -> Vec<HarnessFeature> {
        self.rows()
            .into_iter()
            .filter(|(_, attestation)| attestation.health() == FeatureHealth::Failing)
            .map(|(feature, _)| feature)
            .collect()
    }

    /// Features that fired despite being ablated. A non-empty result
    /// invalidates the run as an experiment: the control arm received the
    /// treatment through a path that does not consult the gate.
    #[must_use]
    pub fn ablation_leaks(&self) -> Vec<HarnessFeature> {
        self.rows()
            .into_iter()
            .filter(|(_, attestation)| attestation.health() == FeatureHealth::AblationLeak)
            .map(|(feature, _)| feature)
            .collect()
    }

    /// True when not a single feature recorded anything — a fresh ledger, or a
    /// run too short to exercise any attested path. Lets a diagnostic surface
    /// say "no data yet" instead of printing an all-zero table that reads like
    /// several separate defects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }
}

/// Process-wide ledger. Shared across every recording site and every spawned
/// sub-agent in this process — deliberately, since a sub-agent's probe firing
/// is as much evidence that the probe works as a foreground one is. A child
/// `zo` process gets its own ledger, which is correct: nothing is inherited
/// through the environment, so this cannot repeat the class of bug where
/// per-session state leaked into a child CLI through an env var.
fn global_ledger() -> &'static Mutex<HarnessAttestLedger> {
    static LEDGER: OnceLock<Mutex<HarnessAttestLedger>> = OnceLock::new();
    LEDGER.get_or_init(|| {
        Mutex::new(HarnessAttestLedger::with_ablation(
            resolved_ablation().set.clone(),
        ))
    })
}

fn with_global<R>(body: impl FnOnce(&mut HarnessAttestLedger) -> R) -> R {
    let mut ledger = global_ledger()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    body(&mut ledger)
}

/// Record one firing of `feature` in the process-wide ledger.
pub fn attest_fired(feature: HarnessFeature) {
    with_global(|ledger| ledger.record_fired(feature));
}

/// Record that `feature` tried and failed, for `reason`. Use this only when
/// something actually went wrong — see [`attest_declined`] for a gate that
/// chose not to run.
pub fn attest_failed(feature: HarnessFeature, reason: &'static str) {
    with_global(|ledger| ledger.record_failed(feature, reason));
}

/// Record that a gate deliberately declined to run `feature`, for `reason`.
/// Declines are normal operation and must never be reported as defects.
pub fn attest_declined(feature: HarnessFeature, reason: &'static str) {
    with_global(|ledger| ledger.record_declined(feature, reason));
}

/// Snapshot the process-wide ledger for a diagnostic surface.
#[must_use]
pub fn harness_attest_snapshot() -> HarnessAttestLedger {
    with_global(|ledger| ledger.clone())
}

/// This process's ablation, resolved once from [`ABLATION_ENV`].
///
/// Unlike the ledger — which is deliberately per-process so a child's counters
/// never masquerade as its parent's — the ablation is CONFIG and is meant to
/// reach children through the ordinary environment. An experiment that
/// suppressed a feature in the orchestrator but not in the agents it spawns
/// would measure a mixture of both arms.
struct ResolvedAblation {
    set: AblationSet,
    /// A malformed spec is kept rather than applied: silently ablating nothing
    /// would let a typo pass for a completed experiment. Callers that can
    /// refuse to run surface this; see [`ablation_spec_error`].
    error: Option<String>,
}

fn resolved_ablation() -> &'static ResolvedAblation {
    static RESOLVED: OnceLock<ResolvedAblation> = OnceLock::new();
    RESOLVED.get_or_init(|| {
        let Some(raw) = std::env::var_os(ABLATION_ENV) else {
            return ResolvedAblation {
                set: AblationSet::none(),
                error: None,
            };
        };
        let Some(spec) = raw.to_str() else {
            return ResolvedAblation {
                set: AblationSet::none(),
                error: Some(format!("{ABLATION_ENV}: value is not valid UTF-8")),
            };
        };
        match AblationSet::parse(spec) {
            Ok(set) => ResolvedAblation { set, error: None },
            Err(error) => ResolvedAblation {
                set: AblationSet::none(),
                error: Some(error.to_string()),
            },
        }
    })
}

/// The features suppressed in this process.
#[must_use]
pub fn ablation_set() -> AblationSet {
    resolved_ablation().set.clone()
}

/// The reason [`ABLATION_ENV`] was rejected, when it was. A caller that can
/// refuse to start should do so on `Some`: the alternative is a run that looks
/// like a control arm and is not one.
#[must_use]
pub fn ablation_spec_error() -> Option<&'static str> {
    resolved_ablation().error.as_deref()
}

/// Consult this process's ablation for `feature`, recording the decline when
/// it is suppressed. `true` means the caller must skip the feature.
#[must_use]
pub fn attest_ablated(feature: HarnessFeature) -> bool {
    if resolved_ablation().set.is_empty() {
        return false;
    }
    with_global(|ledger| ledger.record_ablation(feature))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_feature_is_silent_not_absent() {
        let ledger = HarnessAttestLedger::new();
        let rows = ledger.rows();
        assert_eq!(
            rows.len(),
            HarnessFeature::all().len(),
            "every feature must appear even with nothing recorded"
        );
        for (feature, attestation) in rows {
            assert_eq!(attestation.health(), FeatureHealth::Silent, "{}", feature.key());
        }
    }

    #[test]
    fn repeated_failure_with_no_firing_is_the_defect_signature() {
        // The exact shape of the multi-week probe regression: the path ran
        // every time and produced nothing every time.
        let mut ledger = HarnessAttestLedger::new();
        for _ in 0..16 {
            ledger.record_failed(HarnessFeature::RoutingProbe, "provider_failure");
        }
        let probe = ledger.attestation(HarnessFeature::RoutingProbe);
        assert_eq!(probe.health(), FeatureHealth::Failing);
        assert_eq!(probe.failed_total(), 16);
        assert_eq!(ledger.failing_features(), vec![HarnessFeature::RoutingProbe]);
    }

    #[test]
    fn declines_alone_are_gated_off_never_failing() {
        // A session with no design turn declines the reminder on every turn.
        // Reporting that as a defect is how a diagnostic table loses its
        // reader, so it must land in a different health state entirely.
        let mut ledger = HarnessAttestLedger::new();
        for _ in 0..40 {
            ledger.record_declined(HarnessFeature::DesignGuidance, "intent_not_design");
        }
        let guidance = ledger.attestation(HarnessFeature::DesignGuidance);
        assert_eq!(guidance.health(), FeatureHealth::GatedOff);
        assert_eq!(guidance.declined_total(), 40);
        assert_eq!(guidance.failed_total(), 0);
        assert!(
            ledger.failing_features().is_empty(),
            "an ordinary decline must never be escalated as a failure"
        );
    }

    #[test]
    fn a_single_failure_outranks_many_declines() {
        // Mixed evidence: the gate declined most turns, but one attempt broke.
        // The broken attempt is the finding, so Failing must win.
        let mut ledger = HarnessAttestLedger::new();
        for _ in 0..9 {
            ledger.record_declined(HarnessFeature::RoutingProbe, "not_worth_it");
        }
        ledger.record_failed(HarnessFeature::RoutingProbe, "timeout");
        assert_eq!(
            ledger.attestation(HarnessFeature::RoutingProbe).health(),
            FeatureHealth::Failing
        );
    }

    #[test]
    fn a_single_firing_clears_every_defect_signature() {
        let mut ledger = HarnessAttestLedger::new();
        ledger.record_failed(HarnessFeature::RoutingProbe, "timeout");
        ledger.record_declined(HarnessFeature::RoutingProbe, "not_worth_it");
        ledger.record_fired(HarnessFeature::RoutingProbe);
        let probe = ledger.attestation(HarnessFeature::RoutingProbe);
        assert_eq!(probe.health(), FeatureHealth::Alive);
        assert_eq!(probe.fired, 1);
        assert_eq!(probe.failed_total(), 1, "a firing must not erase the failure history");
        assert_eq!(probe.declined_total(), 1);
        assert!(ledger.failing_features().is_empty());
    }

    #[test]
    fn reasons_are_tallied_per_kind_and_ranked_by_frequency() {
        let mut ledger = HarnessAttestLedger::new();
        for _ in 0..3 {
            ledger.record_failed(HarnessFeature::RoutingProbe, "timeout");
        }
        for _ in 0..5 {
            ledger.record_failed(HarnessFeature::RoutingProbe, "malformed");
        }
        ledger.record_failed(HarnessFeature::RoutingProbe, "join_failure");
        for _ in 0..7 {
            ledger.record_declined(HarnessFeature::RoutingProbe, "not_worth_it");
        }
        let probe = ledger.attestation(HarnessFeature::RoutingProbe);
        assert_eq!(
            probe.failures_by_frequency(),
            vec![("malformed", 5), ("timeout", 3), ("join_failure", 1)],
            "most frequent failure first — that is the one worth fixing"
        );
        assert_eq!(probe.declines_by_frequency(), vec![("not_worth_it", 7)]);
        assert_eq!(probe.failed_total(), 9, "declines must not leak into the failure total");
    }

    #[test]
    fn features_do_not_bleed_into_each_other() {
        let mut ledger = HarnessAttestLedger::new();
        ledger.record_fired(HarnessFeature::ReasoningReplayTurnFinal);
        assert_eq!(ledger.attestation(HarnessFeature::ReasoningReplayTurnFinal).fired, 1);
        assert_eq!(
            ledger.attestation(HarnessFeature::ReasoningReplayCall).health(),
            FeatureHealth::Silent
        );
    }

    #[test]
    fn every_feature_has_a_unique_stable_key_a_label_and_a_precondition() {
        let mut keys: Vec<&str> = HarnessFeature::all().iter().map(|f| f.key()).collect();
        let total = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), total, "feature keys must be unique");
        for feature in HarnessFeature::all() {
            assert!(!feature.key().is_empty());
            assert!(!feature.label().is_empty());
            assert!(
                !feature.precondition().is_empty(),
                "a silent row is only readable next to its precondition: {}",
                feature.key()
            );
        }
    }

    #[test]
    fn every_feature_key_round_trips_through_from_key_case_insensitively() {
        for feature in HarnessFeature::all() {
            assert_eq!(HarnessFeature::from_key(feature.key()), Some(*feature));
            assert_eq!(
                HarnessFeature::from_key(&feature.key().to_ascii_uppercase()),
                Some(*feature),
                "an ablation spec typed in caps must still name {}",
                feature.key()
            );
        }
        assert_eq!(HarnessFeature::from_key("no_such_feature"), None);
    }

    #[test]
    fn an_ablation_spec_parses_keys_tolerating_spacing_and_expands_all() {
        let parsed = AblationSet::parse(" routing_probe , design_guidance ,")
            .expect("spacing and a trailing comma are not errors");
        assert!(parsed.contains(HarnessFeature::RoutingProbe));
        assert!(parsed.contains(HarnessFeature::DesignGuidance));
        assert!(!parsed.contains(HarnessFeature::InformationTopology));
        assert_eq!(parsed.keys(), vec!["routing_probe", "design_guidance"]);

        assert!(AblationSet::parse("").expect("empty spec").is_empty());
        assert_eq!(
            AblationSet::parse("all").expect("all"),
            AblationSet::all(),
            "`all` must expand to every attested feature"
        );
    }

    #[test]
    fn an_unknown_ablation_token_is_an_error_that_names_it_and_the_alternatives() {
        // A typo'd treatment that silently applied nothing would report a
        // completed experiment measuring the production arm twice.
        let error = AblationSet::parse("routing_probe,desgin_guidance")
            .expect_err("a misspelled feature must not be skipped");
        assert_eq!(error.token, "desgin_guidance");
        let rendered = error.to_string();
        assert!(rendered.contains("desgin_guidance"), "{rendered}");
        assert!(rendered.contains("design_guidance"), "{rendered}");
        assert!(rendered.contains(ABLATION_ENV), "{rendered}");
        // The alternatives list is an offer of valid tokens, so a feature that
        // parse would then reject has no business appearing in it.
        assert!(!rendered.contains("steering_reissue"), "{rendered}");
    }

    #[test]
    fn user_initiated_features_cannot_be_held_out() {
        // `all` is the floor arm of an experiment; a user's Esc or mid-turn
        // correction is not part of any experiment, so the floor must not make
        // them silently inert — and their firings must never be able to read
        // as an AblationLeak that voids the run.
        let floor = AblationSet::all();
        assert!(!floor.contains(HarnessFeature::SteeringReissue));
        assert!(!floor.contains(HarnessFeature::ToolCancelSettled));
        assert_eq!(AblationSet::parse("all").expect("all"), floor);

        // Naming one explicitly is a hard error with its own message: the call
        // sites never consult the gate, so accepting the token would run the
        // treatment while reporting a holdout.
        let error = AblationSet::parse("steering_reissue")
            .expect_err("a user-initiated feature is not a valid holdout");
        assert!(error.not_ablatable);
        let rendered = error.to_string();
        assert!(rendered.contains("user-initiated"), "{rendered}");
        assert!(rendered.contains("steering_reissue"), "{rendered}");
        assert!(
            AblationSet::parse("tool_cancel_settled").is_err(),
            "both user-initiated features must be rejected"
        );

        // Every harness-initiated feature still parses — the classification
        // must never quietly shrink the legitimate experiment surface.
        for feature in HarnessFeature::all() {
            assert_eq!(
                AblationSet::parse(feature.key()).is_ok(),
                feature.ablatable(),
                "{} parse admission must equal its ablatable class",
                feature.key()
            );
        }
    }

    #[test]
    fn recording_an_ablation_suppresses_only_the_named_feature() {
        let ablation = AblationSet::parse("info_topology").expect("spec");
        let mut ledger = HarnessAttestLedger::with_ablation(ablation);

        assert!(
            ledger.record_ablation(HarnessFeature::InformationTopology),
            "an ablated feature must report that the caller skips it"
        );
        assert!(
            !ledger.record_ablation(HarnessFeature::RoutingProbe),
            "a feature outside the ablation must run normally"
        );

        let topology = ledger.attestation(HarnessFeature::InformationTopology);
        assert_eq!(topology.health(), FeatureHealth::Ablated);
        assert!(topology.was_ablated());
        assert_eq!(topology.declines_by_frequency(), vec![(ABLATED_REASON, 1)]);
        assert_eq!(
            ledger.attestation(HarnessFeature::RoutingProbe),
            FeatureAttestation::default(),
            "consulting the ablation must not record anything for an unablated feature"
        );
        assert!(ledger.failing_features().is_empty(), "an ablation is not a defect");
        assert!(ledger.ablation_leaks().is_empty());
    }

    #[test]
    fn a_feature_that_fires_while_ablated_invalidates_the_measurement() {
        // The gate was bypassed by some path, so the control arm received the
        // treatment. Reporting this as ordinary `Alive` would publish a
        // difference that the experiment did not actually isolate.
        let mut ledger = HarnessAttestLedger::with_ablation(AblationSet::all());
        assert!(ledger.record_ablation(HarnessFeature::RoutingProbe));
        ledger.record_fired(HarnessFeature::RoutingProbe);

        assert_eq!(
            ledger.attestation(HarnessFeature::RoutingProbe).health(),
            FeatureHealth::AblationLeak
        );
        assert_eq!(ledger.ablation_leaks(), vec![HarnessFeature::RoutingProbe]);
        assert!(
            ledger.failing_features().is_empty(),
            "a leak is its own diagnosis, not a fail-open failure"
        );
    }

    #[test]
    fn a_call_site_that_never_consults_the_gate_is_still_caught_as_a_leak() {
        // The bypass worth catching leaves NO decline behind: a site that
        // forgot the gate entirely records only a firing. Classifying that
        // firing against recorded declines would read it as a healthy `Alive`
        // row — the silent invalid measurement this detection exists for.
        let mut ledger =
            HarnessAttestLedger::with_ablation(AblationSet::parse("info_topology").expect("spec"));
        ledger.record_fired(HarnessFeature::InformationTopology);

        let topology = ledger.attestation(HarnessFeature::InformationTopology);
        assert_eq!(topology.leaked, 1);
        assert!(
            !topology.was_ablated(),
            "the premise of this test is that no decline was ever recorded"
        );
        assert_eq!(topology.health(), FeatureHealth::AblationLeak);
        assert_eq!(
            ledger.ablation_leaks(),
            vec![HarnessFeature::InformationTopology]
        );
    }

    #[test]
    fn a_firing_outside_the_ablation_is_never_marked_as_a_leak() {
        let mut ledger =
            HarnessAttestLedger::with_ablation(AblationSet::parse("routing_probe").expect("spec"));
        ledger.record_fired(HarnessFeature::DesignGuidance);

        let guidance = ledger.attestation(HarnessFeature::DesignGuidance);
        assert_eq!(guidance.leaked, 0);
        assert_eq!(guidance.health(), FeatureHealth::Alive);
        assert!(ledger.ablation_leaks().is_empty());
    }

    #[test]
    fn an_ablation_never_buries_a_real_failure_and_a_failure_is_not_a_leak() {
        // Ablated, never fired, but a path attested a failure. The breakage is
        // the finding, so it must not be masked — AND it must not be promoted
        // to a leak either: `smart_router::turn` attests probe failures
        // upstream of `probe_exec`'s gate, so a failure under a holdout is not
        // evidence that anything bypassed the gate.
        let mut ledger = HarnessAttestLedger::with_ablation(AblationSet::all());
        assert!(ledger.record_ablation(HarnessFeature::DesignGuidance));
        ledger.record_failed(HarnessFeature::DesignGuidance, "settings_unavailable");

        let guidance = ledger.attestation(HarnessFeature::DesignGuidance);
        assert_eq!(guidance.leaked, 0, "a failure is not a firing");
        assert_eq!(guidance.health(), FeatureHealth::Failing);
        assert_eq!(ledger.failing_features(), vec![HarnessFeature::DesignGuidance]);
        assert!(
            ledger.ablation_leaks().is_empty(),
            "an upstream-attested failure must not condemn a sound control arm"
        );
    }

    #[test]
    fn an_unablated_process_reports_no_suppression_and_no_spec_error() {
        // The default arm: nothing set, nothing suppressed. Asserted through
        // the process-wide accessors so a stray ablation in the test
        // environment would surface here rather than skewing a measurement.
        assert!(ablation_set().is_empty(), "tests must run in the production arm");
        assert_eq!(ablation_spec_error(), None);
        assert!(
            !attest_ablated(HarnessFeature::InformationTopology),
            "no feature may be suppressed without ZO_ABLATE"
        );
    }

    #[test]
    fn empty_reports_emptiness_until_something_is_recorded() {
        let mut ledger = HarnessAttestLedger::new();
        assert!(ledger.is_empty(), "a fresh ledger has no data to report");
        ledger.record_declined(HarnessFeature::RoutingProbe, "not_worth_it");
        assert!(!ledger.is_empty(), "a decline is data");
    }

    #[test]
    fn the_global_ledger_receives_every_recording_kind() {
        // The global is process-wide, so this asserts only DELTAS, never
        // absolute counts — a sibling test touching the same feature must not
        // be able to break it.
        let before = harness_attest_snapshot().attestation(HarnessFeature::DesignGuidance);
        attest_fired(HarnessFeature::DesignGuidance);
        attest_failed(HarnessFeature::DesignGuidance, "test_failure");
        attest_declined(HarnessFeature::DesignGuidance, "test_decline");
        let after = harness_attest_snapshot().attestation(HarnessFeature::DesignGuidance);
        assert!(after.fired > before.fired, "attest_fired must reach the global ledger");
        assert!(
            after.failed_total() > before.failed_total(),
            "attest_failed must reach the global ledger"
        );
        assert!(
            after.declined_total() > before.declined_total(),
            "attest_declined must reach the global ledger"
        );
    }

    /// Every attested feature must have a place it can FIRE, outside tests.
    ///
    /// This is the check that would have caught the reasoning-replay defect on
    /// day one: the payload it read was always empty on the live backend, so
    /// the feature could never fire — and nothing in a green suite said so,
    /// because the ledger's own row is written from the call site that no
    /// longer ran. A variant with `attest_declined` sites but no `attest_fired`
    /// site is a feature whose ledger row can only ever read "gated".
    ///
    /// Deliberately a source scan rather than a call-graph analysis: the
    /// property is "a human wired this axis somewhere real", and a grep for the
    /// firing call is exactly that question. It costs one directory walk and
    /// runs on every `cargo test`.
    #[test]
    fn every_harness_feature_has_a_non_test_firing_site() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let mut sources = Vec::new();
        collect_rust_sources(&workspace, &mut sources);
        assert!(
            sources.len() > 50,
            "the scan must actually find the tree, got {} files",
            sources.len()
        );

        let mut fired: std::collections::BTreeMap<&str, usize> = HarnessFeature::all()
            .iter()
            .map(|feature| (feature.key(), 0usize))
            .collect();
        for path in &sources {
            // Test modules are excluded by FILE, not by parsing `cfg(test)`:
            // a firing site that only exists inside a unit-test module is
            // exactly what this test is meant to reject, and the repo keeps
            // its tests in `tests.rs` / `tests/` by convention.
            let name = path.to_string_lossy();
            if name.ends_with("tests.rs") || name.contains("/tests/") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            for feature in HarnessFeature::all() {
                // `Debug` prints the variant identifier, which is what a call site spells.
                let variant = format!("HarnessFeature::{feature:?}");
                for (index, _) in text.match_indices(&variant) {
                    let before = &text[index.saturating_sub(80)..index];
                    if before.contains("attest_fired") {
                        *fired.get_mut(feature.key()).expect("known key") += 1;
                    }
                }
            }
        }

        let dark: Vec<&str> = fired
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(key, _)| *key)
            .collect();
        assert!(
            dark.is_empty(),
            "these features can never fire — no `attest_fired` outside test files: {dark:?}. \
             Either wire the firing site or remove the variant; a feature that can only \
             record declines is a row that will read 'gated' forever."
        );
    }

    fn collect_rust_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                collect_rust_sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
}
