//! Aggregate-safe Smart Router outcome history.
//!
//! This module owns the small persisted signal that later phases can use to
//! evaluate routing quality. It deliberately stores only route/model/status
//! metadata and bounded counters — never prompts, agent output, or raw errors.

use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::policy::{RouteFeedbackHint, MAX_FEEDBACK_ADJUSTMENT};
use crate::jsonl_log::rewrite_jsonl_lines_if_changed;

/// Decisive-sample count at which outcome feedback reaches its full weight. Below
/// this, the adjustment ramps up linearly with the sample count so a thin history
/// (the `>=2` minimum) only nudges, while an well-evidenced route can override the
/// recency prior. Sized so ~8 decisive runs earn full trust.
///
/// `pub` (not `pub(super)`): also the exploration-eligibility threshold
/// (`policy::select_ranked_auto_candidate`'s "incumbent >=8 decisive" gate)
/// and the `/smart doctor` exploration-status section's eligibility check
/// (P7) both need the SAME number a caller outside this module can name,
/// rather than a silently-duplicated magic `8`.
pub const CONFIDENT_DECISIVE_SAMPLES: i32 = 8;
const OUTCOME_DIR: &str = "smart-router";
const OUTCOME_FILE: &str = "route-outcomes.jsonl";

/// Per-`(route_key, selectedModel)` bucket cap (P3 retention v2). Replaces the
/// old flat [`OUTCOME_GLOBAL_RETENTION`]-only cap, which let a single
/// high-traffic route (observed live: `code-reviewer:gpt-5.5-fast` at 195+
/// decisive samples) crowd out a low-traffic route's ENTIRE history before it
/// ever reached the `>=2` decisive minimum. A bucket over this cap drops its
/// oldest records first; append order is otherwise preserved.
const OUTCOME_BUCKET_RETENTION: usize = 48;
/// Global cap across all buckets (P3 retention v2), replacing the old flat
/// 512-record cap. Sized well above `OUTCOME_BUCKET_RETENTION` times a
/// realistic number of concurrently-active routes, so it only engages when
/// the store holds many distinct routes at once. When exceeded, the
/// globally-oldest record is evicted from whichever bucket is CURRENTLY
/// LARGEST, repeated until back under budget — so a handful of hot routes
/// absorb the trim and a thin, low-traffic route's history is the last thing
/// cut.
const OUTCOME_GLOBAL_RETENTION: usize = 2048;
/// Half-life (in days) for [`weighted_feedback_hint_for_route_key`]'s
/// recency weighting: a decisive record's contribution to the confidence-
/// weighted adjustment halves every this many days. Sized so a signal from
/// the project's typical ~1-week outcome-store window still counts heavily,
/// while a month-old-only bucket has decayed toward near-zero influence
/// instead of freezing at the full bound forever.
const FEEDBACK_HALF_LIFE_DAYS: f64 = 14.0;

/// The route key every [`DecisionKind::Classify`] row is filed under. The tax
/// of deciding is not evidence about any work route, so it has a key of its
/// own and never lands in a work route's feedback bucket.
pub const ROUTE_TAX_ROUTE_KEY: &str = "route-tax";

/// The orchestration decisions the accuracy roadmap learns over — not just
/// "which model" (the original outcome axis) but the full surface: whether to
/// route to a different AGENT, whether to DECOMPOSE into lanes, whether to
/// VERIFY a spawn's output, whether a spawn was FOLDED as wasted, and what the
/// host paid to CLASSIFY before deciding. One table ([`DecisionKind::ALL`]) is
/// the single source of truth for the set, so adding a kind is a one-line
/// change here rather than a literal sprinkled across writers (the
/// no-hardcoding contract, see `decision_kind_registry_is_the_single_source_of_kinds`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionKind {
    Model,
    Agent,
    Decompose,
    Verify,
    Fold,
    /// The routing tax — a classification call the host paid before it
    /// decided: the difficulty probe or the fan-out decomposition call. The
    /// row's status is the call's own (completed, failed, stopped on a
    /// timeout), its `duration_ms`/`output_tokens` are the tax, its `run_id`
    /// names the attempt that paid it, its `target` says which call
    /// (`probe`/`decompose`) and its route key is [`ROUTE_TAX_ROUTE_KEY`]. It
    /// never teaches the router (the learning mask skips it like a fold) and
    /// never counts as a work decision in the accuracy report; it exists so a
    /// plan's total cost can include the cost of deciding.
    Classify,
}

impl DecisionKind {
    /// Every decision kind, in canonical display order. The ONE place the set
    /// is enumerated.
    pub const ALL: [DecisionKind; 6] = [
        Self::Model,
        Self::Agent,
        Self::Decompose,
        Self::Verify,
        Self::Fold,
        Self::Classify,
    ];

    /// Canonical label persisted in a record's `decision` field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Agent => "agent",
            Self::Decompose => "decompose",
            Self::Verify => "verify",
            Self::Fold => "fold",
            Self::Classify => "classify",
        }
    }

    /// Whether a row of this kind is bookkeeping rather than an attempt at
    /// work — a fold (a lane the merge discarded) or a classify (the tax of
    /// deciding). Neither teaches the router and neither is a first try that
    /// could succeed; the one predicate both the learning mask and the
    /// accuracy report ask.
    #[must_use]
    pub fn is_bookkeeping(self) -> bool {
        matches!(self, Self::Fold | Self::Classify)
    }

    /// Parse a persisted label back to a kind; `None` for an unknown/foreign
    /// string (the aggregator keeps such a row under its raw label rather than
    /// silently folding it into `Model`).
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == label)
    }
}

/// The plan shape a turn or a spawn ran under — how the work was laid out,
/// not which model did it: alone, split by the host's pre-analysis into
/// `width` read-only lanes, handed to one delegate, or run as `width` parallel
/// lanes the model asked for. The label grammar is `<name>[:<width>]`; the
/// four names live here and nowhere else. The outcome record stores the label
/// so a later scorer can learn "did the split pay" per shape, next to the
/// model and the effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanShape {
    Solo,
    HostPrelude { width: u32 },
    Delegate,
    Parallel { width: u32 },
}

impl PlanShape {
    const SOLO: &'static str = "solo";
    const HOST_PRELUDE: &'static str = "host-prelude";
    const DELEGATE: &'static str = "delegate";
    const PARALLEL: &'static str = "parallel";

    /// Canonical label persisted in a record's `shape` field.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Solo => Self::SOLO.to_string(),
            Self::HostPrelude { width } => format!("{}:{width}", Self::HOST_PRELUDE),
            Self::Delegate => Self::DELEGATE.to_string(),
            Self::Parallel { width } => format!("{}:{width}", Self::PARALLEL),
        }
    }

    /// Parse a persisted label; `None` for an unknown name, a lane shape with
    /// no width, a width on a shape that has none, or a width below one (a
    /// split into zero lanes is no split).
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        let (name, width) = match label.split_once(':') {
            Some((name, width)) => (name, Some(width.parse::<u32>().ok().filter(|w| *w >= 1)?)),
            None => (label, None),
        };
        match (name, width) {
            (Self::SOLO, None) => Some(Self::Solo),
            (Self::DELEGATE, None) => Some(Self::Delegate),
            (Self::HOST_PRELUDE, Some(width)) => Some(Self::HostPrelude { width }),
            (Self::PARALLEL, Some(width)) => Some(Self::Parallel { width }),
            _ => None,
        }
    }

    /// Lanes this shape runs at once: one for solo and delegate, `width` for
    /// the lane shapes.
    #[must_use]
    pub fn lanes(self) -> u32 {
        match self {
            Self::Solo | Self::Delegate => 1,
            Self::HostPrelude { width } | Self::Parallel { width } => width,
        }
    }
}

/// The classification calls a host pays for before it decides — the two
/// targets a [`DecisionKind::Classify`] row can name.
///
/// One table, like every other label set here: a writer names a call by
/// variant and never spells the string, so the tax ledger cannot grow a third
/// spelling of the same call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteTaxCall {
    /// The difficulty probe: one bounded Fast-tier call before a route is
    /// chosen.
    Probe,
    /// The fan-out decomposition call that decides the lanes.
    Decompose,
}

impl RouteTaxCall {
    /// Both calls, the one place the set is enumerated.
    pub const ALL: [RouteTaxCall; 2] = [Self::Probe, Self::Decompose];

    /// Canonical label persisted in a tax row's `target` field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Decompose => "decompose",
        }
    }

    /// Parse a persisted label; `None` for an unknown string.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|call| call.as_str() == label)
    }
}

/// What settled a verification verdict (orchestration-accuracy P2): an
/// OBJECTIVE signal — a command/gate/compile/test result — or MODEL judgement,
/// an LLM's opinion. The accuracy dashboard reads this to show what fraction of
/// "verify passed" is only an LLM opinion; a completion loop that drives to a
/// model-only green is the deepest form of appearance-over-results the design's
/// honest-risk section warns about, so the basis must be visible, not assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerdictBasis {
    Objective,
    Model,
}

impl VerdictBasis {
    /// Both bases, the one place the set is enumerated.
    pub const ALL: [VerdictBasis; 2] = [Self::Objective, Self::Model];

    /// Canonical label persisted in a record's `verdict_basis` field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Objective => "objective",
            Self::Model => "model",
        }
    }

    /// Parse a persisted label; `None` for an unknown string.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|basis| basis.as_str() == label)
    }
}

/// The single rule that decides a verdict's basis: a verdict backed by an
/// objective signal is [`VerdictBasis::Objective`], otherwise
/// [`VerdictBasis::Model`]. A pure seam so a future objective-verdict recorder
/// and the existing model-judgement recorders name the basis the same way.
#[must_use]
pub fn resolve_verdict_basis(objective_backed: bool) -> VerdictBasis {
    if objective_backed {
        VerdictBasis::Objective
    } else {
        VerdictBasis::Model
    }
}

/// WHO a failed verify verdict is about. A validator normally judges the
/// WORK; but when the validator's own output is unusable, the engine records
/// a failed verdict against the validator itself (attribution's `Invalid`
/// arm). Without this label the two are indistinguishable and every such
/// validator fault inflates the "defects caught" count (re-verification
/// finding, 2026-09-10). Absent == `Work`, the historical meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerdictSubject {
    Work,
    Validator,
}

impl VerdictSubject {
    /// Both subjects, the one place the set is enumerated.
    pub const ALL: [VerdictSubject; 2] = [Self::Work, Self::Validator];

    /// Canonical label persisted in a record's `verdict_subject` field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Validator => "validator",
        }
    }

    /// Parse a persisted label; `None` for an unknown string.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|subject| subject.as_str() == label)
    }
}

// NOTE: `PartialEq` only (not `Eq`) — `signal_weight: Option<f32>` (P3 schema
// v2) has no total ordering, so the whole struct cannot derive `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteOutcomeRecord {
    pub recorded_at: u64,
    pub route_key: String,
    pub target_kind: String,
    pub target: String,
    pub selected_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error_class: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub output_tokens: u64,
    /// Evidence source: absent for run-level outcomes (the spawn recorder),
    /// `"verdict"` for cross-check attribution — a validator's judgement of a
    /// worker's output folded back onto the worker's model. Provenance only;
    /// aggregation treats both as decisive samples (run outcomes say "the
    /// model finished", verdict outcomes say "the work was right") — except
    /// that a settled verdict about an attributed attempt (`run_id`) speaks
    /// for that attempt INSTEAD of its run outcome, since "the model
    /// finished" only ever stood in for the question the verdict answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    /// The verification model that judged this record's `selected_model`, for a
    /// `signal:"verdict"` pair-attribution record: `(selected_model =
    /// implementation model, verifier_model = the model that cross-checked it)`.
    /// Populated only on the main-turn verdict leg when a cross-model verifier
    /// actually ran; `None` for run outcomes, native same-model verify, and
    /// every pre-v2 record. Recorded for provenance/`/smart doctor` only — no
    /// scorer learns from the pair key yet. Optional + serde-default so pre-v2
    /// lines still deserialize (see `route_outcome_v2_schema_parses_pre_v2_live_records`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_model: Option<String>,
    // --- P3 schema v2: all OPTIONAL, serde-default so every pre-v2 record on
    // disk (see the `route_outcome_v2_schema_parses_pre_v2_live_records` test,
    // built from real captured lines) still deserializes with `None`/`0`.
    // Populated only by routes the Smart router actually classified — a
    // record with these absent means routing-off, an explicit model, or a
    // pre-v2 write, never a parse failure.
    /// Smart-router role label (`RouteRole`, e.g. `"coding"`/`"analysis"`) at
    /// the time this route was decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Task complexity (`RouteTaskComplexity`, lowercased) at decision time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    /// Task risk (`RouteTaskRisk`, lowercased) at decision time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    /// Reasoning-effort tier actually used for the run, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_level: Option<String>,
    /// Wall-clock run duration in milliseconds, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// How this route was decided: `"auto"` | `"pin"` | `"explicit"` |
    /// `"fallback"` | `"exploration"`. Distinguishes an AUTO-selector pick
    /// (real routing-quality signal) from a config-forced pin/family-selector
    /// override (an AVAILABILITY signal, not a quality one — the live store's
    /// largest bucket was a pin's residue, per the routing plan's live-data
    /// audit) so a later learning phase can weight them differently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_source: Option<String>,
    /// What the routing probe said its OWN confidence was (`"low"` |
    /// `"medium"` | `"high"`), verbatim from the model's reply — not the
    /// router's signal-weight confidence, which is a different number derived
    /// from the deterministic classifier's evidence.
    ///
    /// Recorded whenever a probe ARRIVED and parsed, including the `low`
    /// verdicts fusion discards (`fuse_probe_assessment`'s gate). That is the
    /// point: `routeSource` already says whether a probe INFORMED the route
    /// (`auto+probe`), so without this field a declined `low` probe is
    /// indistinguishable from no probe at all — and the `low` bucket is
    /// exactly the one a calibration read needs, since the whole question is
    /// whether the probe's stated confidence predicts whether it was right.
    ///
    /// Provenance only: nothing routes on it. It exists so the claimed
    /// confidence can be checked against decisive outcomes before the
    /// hand-set fusion gate (high = +/-1 band, medium = raise only) is
    /// replaced by a measured one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_confidence: Option<String>,
    /// Reserved for a future weighted-signal source (e.g. a verdict weighted
    /// differently from a run completion). Not populated or consumed yet;
    /// the field exists now so the schema shape is fixed before a consumer
    /// lands (mirrors the `TiersProvenance::Learned` reservation pattern).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_weight: Option<f32>,
    // --- P1 decision-surface (orchestration-accuracy roadmap): which KIND of
    // orchestration decision this record is evidence for, plus two outcome
    // refinements the accuracy dashboard reads. All OPTIONAL + serde-default,
    // so every pre-existing record parses (a `None` decision reads as
    // `DecisionKind::Model`, the historical default — see `decision_kind`).
    /// The orchestration decision this outcome judges: one of
    /// [`DecisionKind`]'s canonical labels. Absent on model-selection records
    /// written before this axis existed (they ARE model decisions), so `None`
    /// == `DecisionKind::Model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// Fan-out width for a `decompose` decision (how many lanes it split into).
    /// Meaningful only when `decision == "decompose"`; `None` everywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_width: Option<u32>,
    /// An EXPLICIT rework mark (a person or an external tool saying "this run's
    /// work had to be redone"). Reserved: no recorder writes it yet, and the
    /// accuracy report's rework rate reads verify catches instead — the catch
    /// is what triggers a repair round, so a second record for the same run
    /// would count it twice. Per-kind tallies still surface the mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reworked: Option<bool>,
    /// A human redirected this decision (steered the plan, swapped the model,
    /// cancelled a fan-out) — an intervention, not an autonomous outcome. A
    /// later phase keeps it out of the autonomous-accuracy numerator; absent
    /// when the decision ran autonomously to its outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intervened: Option<bool>,
    /// What settled a `verify` decision's verdict — a [`VerdictBasis`] label
    /// (`objective`/`model`). Absent on non-verify records and on verify
    /// records written before this axis; a `None` on a verify record reads as
    /// `model` (a bare verdict is an LLM judgement). See `verdict_basis_kind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_basis: Option<String>,
    /// Who a failed verify verdict is about — a [`VerdictSubject`] label
    /// (`work`/`validator`). Absent on every record before this axis and on
    /// every verdict about the work itself; `validator` marks the engine's
    /// "the validator's own output was unusable" failure so it never counts as
    /// a caught defect. See `verdict_subject_kind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_subject: Option<String>,
    /// The spawn attempt this record is evidence about, `<agentId>#<runGeneration>`
    /// (see [`RouteOutcomeRecord::with_attempt`]): the spawn recorder stamps
    /// the attempt that just ran, the verdict and fold recorders stamp the
    /// attempt they JUDGED — the implementer's, never the verifier job's own.
    /// The pair is the agent store's durable identity: the id is wall-clock
    /// nanoseconds claimed with an exclusive create, and the generation lives
    /// in the manifest and advances on every resume. Provenance only, read by
    /// `learning_sample_mask` so one attempt teaches the router once. Absent
    /// on every record before this field and on main-turn verdicts (no spawn
    /// attempt to name) — such a record stays unattributed and counts exactly
    /// as it did before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The attempt this record's work SERVED — the turn or spawn that asked
    /// for it: a main turn's spawn names `<sessionId>@<turn>`, a verifier
    /// names the attempt it judged, a retry names the original attempt, a
    /// pre-analysis lane and a classify row name the parent turn. The
    /// total-cost join key: every request row and outcome row whose attempt
    /// or parent is one key, summed, prices that unit of work — implementer,
    /// verifier, lanes and the tax of deciding together. Absent on every
    /// record before this field and on work nothing asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_attempt: Option<String>,
    /// The [`PlanShape`] label the work ran under (`solo`, `host-prelude:<w>`,
    /// `delegate`, `parallel:<w>`); absent before this field and where the
    /// recorder did not know the shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
}

/// A key as stored: trimmed, and absent when nothing is left.
fn non_empty_key(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

impl RouteOutcomeRecord {
    #[must_use]
    pub fn new(
        target_kind: impl Into<String>,
        target: impl Into<String>,
        selected_model: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        let target_kind = target_kind.into();
        let target = target.into();
        Self {
            recorded_at: epoch_seconds_now(),
            route_key: format!("{target_kind}:{target}"),
            target_kind,
            target,
            selected_model: selected_model.into(),
            requested_model: None,
            status: status.into(),
            provider_error_class: None,
            output_tokens: 0,
            signal: None,
            verifier_model: None,
            role: None,
            complexity: None,
            risk: None,
            effort_level: None,
            duration_ms: None,
            route_source: None,
            probe_confidence: None,
            signal_weight: None,
            decision: None,
            decision_width: None,
            reworked: None,
            intervened: None,
            verdict_basis: None,
            verdict_subject: None,
            run_id: None,
            parent_attempt: None,
            shape: None,
        }
    }

    /// Stamp the spawn attempt this record is evidence about: the agent id
    /// and the run generation that attempt ran as, spelled by
    /// [`super::spawn_attempt_key`] — the one place that grammar lives, so
    /// this recorder, the verdict recorder and the child runtime that spends
    /// requests under the key all name the same attempt the same way. A blank
    /// id names nothing and leaves the record unattributed.
    #[must_use]
    pub fn with_attempt(mut self, agent_id: &str, run_generation: u64) -> Self {
        self.run_id = super::spawn_attempt_key(agent_id, run_generation);
        self
    }

    /// The routing tax one classification call cost, filed under
    /// [`ROUTE_TAX_ROUTE_KEY`].
    ///
    /// The ONE constructor for a [`DecisionKind::Classify`] row, so no writer
    /// spells the decision label or the route key. The tax is not evidence
    /// about any work route — a probe that answered in 900 ms says nothing
    /// about whether the model it named was a good choice — so it gets a key
    /// of its own and never lands in a work route's feedback bucket. The
    /// caller adds the attempt that paid (`with_attempt_key`), the wall clock
    /// (`with_duration_ms`) and the tokens (`with_output_tokens`).
    #[must_use]
    pub fn route_tax(
        call: RouteTaxCall,
        selected_model: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        let mut record = Self::new(
            DecisionKind::Classify.as_str(),
            call.as_str(),
            selected_model,
            status,
        );
        record.route_key = ROUTE_TAX_ROUTE_KEY.to_string();
        record.decision = Some(DecisionKind::Classify.as_str().to_string());
        record
    }

    /// Stamp an attempt key already spelled out — a main turn's
    /// `<sessionId>@<turn>`, or a spawn key handed down verbatim. Trimmed; an
    /// empty key leaves the record unattributed. Pairs with
    /// [`Self::with_attempt`], which spells the spawn grammar itself. Readers
    /// never parse either grammar: they join on equality.
    #[must_use]
    pub fn with_attempt_key(mut self, key: impl AsRef<str>) -> Self {
        self.run_id = non_empty_key(key.as_ref());
        self
    }

    /// Stamp the attempt this work served (see [`Self::parent_attempt`]).
    #[must_use]
    pub fn with_parent_attempt(mut self, key: impl AsRef<str>) -> Self {
        self.parent_attempt = non_empty_key(key.as_ref());
        self
    }

    /// Stamp the plan shape the work ran under.
    #[must_use]
    pub fn with_shape(mut self, shape: PlanShape) -> Self {
        self.shape = Some(shape.label());
        self
    }

    /// Stamp a plan shape by its persisted label — the form a slot or a
    /// ledger hands back. A label the grammar does not own stamps nothing,
    /// so the column never carries a string [`PlanShape::from_label`] would
    /// not read back.
    #[must_use]
    pub fn with_shape_label(mut self, label: impl AsRef<str>) -> Self {
        if let Some(shape) = PlanShape::from_label(label.as_ref()) {
            self.shape = Some(shape.label());
        }
        self
    }

    /// The plan shape of this record, when it carries a known label.
    #[must_use]
    pub fn shape_kind(&self) -> Option<PlanShape> {
        self.shape.as_deref().and_then(PlanShape::from_label)
    }

    /// Stamp what settled a verify verdict — objective signal or model
    /// judgement (P2).
    #[must_use]
    pub fn with_verdict_basis(mut self, basis: VerdictBasis) -> Self {
        self.verdict_basis = Some(basis.as_str().to_string());
        self
    }

    /// The basis of this record's verdict, mapping an absent or unknown label
    /// to [`VerdictBasis::Model`] (a bare verdict is an LLM judgement).
    #[must_use]
    pub fn verdict_basis_kind(&self) -> VerdictBasis {
        self.verdict_basis
            .as_deref()
            .and_then(VerdictBasis::from_label)
            .unwrap_or(VerdictBasis::Model)
    }

    /// Stamp who a failed verify verdict is about (P2 re-verification).
    #[must_use]
    pub fn with_verdict_subject(mut self, subject: VerdictSubject) -> Self {
        self.verdict_subject = Some(subject.as_str().to_string());
        self
    }

    /// The subject of this record's verdict, mapping an absent or unknown label
    /// to [`VerdictSubject::Work`] (a verdict is about the work unless said).
    #[must_use]
    pub fn verdict_subject_kind(&self) -> VerdictSubject {
        self.verdict_subject
            .as_deref()
            .and_then(VerdictSubject::from_label)
            .unwrap_or(VerdictSubject::Work)
    }

    /// Stamp the orchestration decision this record is evidence for (P1).
    #[must_use]
    pub fn with_decision(mut self, decision: DecisionKind) -> Self {
        self.decision = Some(decision.as_str().to_string());
        self
    }

    /// Fan-out width for a `decompose` decision; `None` clears it.
    #[must_use]
    pub fn with_decision_width(mut self, width: Option<u32>) -> Self {
        self.decision_width = width;
        self
    }

    /// Mark that a later run reworked the same route (the work needed a fix).
    #[must_use]
    pub fn with_reworked(mut self, reworked: bool) -> Self {
        self.reworked = Some(reworked);
        self
    }

    /// Mark that a human intervened on this decision.
    #[must_use]
    pub fn with_intervened(mut self, intervened: bool) -> Self {
        self.intervened = Some(intervened);
        self
    }

    /// The decision kind this record judges. An explicit `decision` label wins;
    /// absent, a legacy `signal:"verdict"` record — every verdict written
    /// before the decision axis existed, 457 live records at re-verification —
    /// IS a Verify decision in the pre-P1 vocabulary; anything else is
    /// [`DecisionKind::Model`], the historical default. Without the signal
    /// inference the whole historical verify sample would silently fold into
    /// `model` and the accuracy report would show no verification at all.
    #[must_use]
    pub fn decision_kind(&self) -> DecisionKind {
        if let Some(kind) = self.decision.as_deref().and_then(DecisionKind::from_label) {
            return kind;
        }
        if self.signal.as_deref() == Some(VERDICT_SIGNAL) {
            return DecisionKind::Verify;
        }
        DecisionKind::Model
    }

    #[must_use]
    pub fn with_signal(mut self, signal: impl Into<String>) -> Self {
        self.signal = Some(signal.into());
        self
    }

    /// Attach the verification model that judged this record's `selected_model`
    /// (the `(implementation, verifier)` pair, P1). Blank/whitespace is dropped
    /// to `None` — same empty-filtering convention as `with_requested_model`.
    #[must_use]
    pub fn with_verifier_model(mut self, verifier_model: Option<String>) -> Self {
        self.verifier_model = verifier_model.filter(|model| !model.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_requested_model(mut self, requested_model: Option<String>) -> Self {
        self.requested_model = requested_model.filter(|model| !model.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_provider_error_class(mut self, provider_error_class: Option<String>) -> Self {
        self.provider_error_class = provider_error_class.filter(|class| !class.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_output_tokens(mut self, output_tokens: u64) -> Self {
        self.output_tokens = output_tokens;
        self
    }

    #[must_use]
    pub fn with_role(mut self, role: Option<String>) -> Self {
        self.role = role.filter(|value| !value.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_complexity(mut self, complexity: Option<String>) -> Self {
        self.complexity = complexity.filter(|value| !value.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_risk(mut self, risk: Option<String>) -> Self {
        self.risk = risk.filter(|value| !value.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_effort_level(mut self, effort_level: Option<String>) -> Self {
        self.effort_level = effort_level.filter(|value| !value.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_duration_ms(mut self, duration_ms: Option<u64>) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    #[must_use]
    pub fn with_route_source(mut self, route_source: Option<String>) -> Self {
        self.route_source = route_source.filter(|value| !value.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_probe_confidence(mut self, probe_confidence: Option<String>) -> Self {
        self.probe_confidence = probe_confidence.filter(|value| !value.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_signal_weight(mut self, signal_weight: Option<f32>) -> Self {
        self.signal_weight = signal_weight;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteOutcomeSummary {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub stopped: usize,
    pub still_running: usize,
    pub output_tokens: u64,
    pub by_route: Vec<RouteOutcomeBucket>,
}

impl RouteOutcomeSummary {
    #[must_use]
    pub fn feedback_hint_for_route_key(&self, route_key: &str) -> RouteFeedbackHint {
        let buckets: Vec<&RouteOutcomeBucket> = self
            .by_route
            .iter()
            .filter(|bucket| bucket.route_key == route_key)
            .collect();
        // The route's cheapest evidenced model is the price everyone else is
        // measured against (P1-1, 2026-09-10): same success, dearer tokens →
        // a bounded tiebreak against the dearer one.
        let cheapest_mean = buckets
            .iter()
            .filter_map(|bucket| bucket.mean_output_tokens())
            .fold(None, |best: Option<f64>, mean| Some(best.map_or(mean, |b| b.min(mean))));
        buckets
            .iter()
            .filter_map(|bucket| {
                let (model, margin) = bucket.feedback_adjustment()?;
                let cost = cheapest_mean.map_or(0, |cheapest| bucket.cost_adjustment(cheapest));
                Some((model, margin.saturating_add(cost)))
            })
            .fold(RouteFeedbackHint::disabled(), |hint, (model, adjustment)| {
                hint.with_model_adjustment(model, adjustment)
            })
    }

    /// Per-canonical-model decisive (`completed + failed`) sample counts for
    /// a `route_key` — Phase 5's eligibility input. Exposed here so `apply.rs`
    /// can determine exploration eligibility straight off the already-loaded
    /// summary, with no second JSONL read. `selected_model` is already
    /// canonical whenever this summary was built via
    /// [`summarize_route_outcomes_with_canonicalizer`] (the live call site,
    /// P3) — this fn does no canonicalization itself, it only reads the
    /// bucket key the summary already computed.
    #[must_use]
    pub fn decisive_counts_for_route_key(&self, route_key: &str) -> Vec<(String, usize)> {
        self.by_route
            .iter()
            .filter(|bucket| bucket.route_key == route_key)
            .map(|bucket| (bucket.selected_model.clone(), bucket.completed.saturating_add(bucket.failed)))
            .collect()
    }

    /// Total records (every status, not just decisive) ever seen for a
    /// `route_key` — the Phase 5 exploration cadence input
    /// (`total % smart.explorationCadence == 0`). Sums `RouteOutcomeBucket::total`
    /// across every model bucket sharing the `route_key`, so it reflects the
    /// SAME retained history `decisive_counts_for_route_key` reads (subject
    /// to the same P3 bucket/global caps).
    #[must_use]
    pub fn total_records_for_route_key(&self, route_key: &str) -> usize {
        self.by_route
            .iter()
            .filter(|bucket| bucket.route_key == route_key)
            .map(|bucket| bucket.total)
            .fold(0usize, usize::saturating_add)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteOutcomeBucket {
    pub route_key: String,
    pub target_kind: String,
    pub target: String,
    pub selected_model: String,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub stopped: usize,
    pub output_tokens: u64,
    /// Records that carried an `outputTokens` figure — the denominator of the
    /// bucket's mean cost. Older records carry none, so `total` would
    /// understate a model that has been around longer.
    #[serde(default)]
    pub output_token_samples: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_errors: BTreeMap<String, usize>,
}

/// Cost is a tiebreak, never a verdict: the most a model can lose for being
/// expensive is half of what evidence can move, so a clear success margin
/// still wins over a cheaper model that fails.
pub const MAX_COST_ADJUSTMENT: i16 = MAX_FEEDBACK_ADJUSTMENT / 2;
/// Output-token samples a bucket needs before its mean cost counts, as a
/// reference or as a penalty. Two, like the decisive floor above.
pub const COST_MIN_SAMPLES: usize = 2;
/// A mean within this fraction above the route's cheapest evidenced model is
/// the same price — provider accounting jitter, not a decision.
pub const COST_TOLERANCE: f64 = 0.25;
/// At this multiple of the cheapest mean the penalty is the full
/// `MAX_COST_ADJUSTMENT`; between the tolerance and here it ramps linearly.
/// Three: on this repository the wandering fast model cost 2.6× the steady
/// one for the same reconnaissance (20.8k vs 8.0k median output tokens).
pub const COST_FULL_RATIO: f64 = 3.0;

impl RouteOutcomeBucket {
    /// Mean output tokens per record that reported them, or `None` below the
    /// sample floor.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn mean_output_tokens(&self) -> Option<f64> {
        (self.output_token_samples >= COST_MIN_SAMPLES)
            .then(|| self.output_tokens as f64 / self.output_token_samples as f64)
    }

    /// The cost tiebreak against the route's cheapest evidenced mean: zero
    /// within `COST_TOLERANCE`, ramping to `-MAX_COST_ADJUSTMENT` at
    /// `COST_FULL_RATIO`. Never positive — being the cheapest earns nothing,
    /// being dearer costs.
    #[must_use]
    pub fn cost_adjustment(&self, cheapest_mean: f64) -> i16 {
        let Some(mean) = self.mean_output_tokens() else {
            return 0;
        };
        if cheapest_mean <= 0.0 {
            return 0;
        }
        let ratio = mean / cheapest_mean;
        let start = 1.0 + COST_TOLERANCE;
        if ratio <= start {
            return 0;
        }
        let span = (COST_FULL_RATIO - start).max(f64::EPSILON);
        let fraction = ((ratio - start) / span).min(1.0);
        let max = f64::from(MAX_COST_ADJUSTMENT);
        #[allow(clippy::cast_possible_truncation)]
        let penalty = (max * fraction).round().clamp(0.0, max) as i16;
        -penalty
    }

    fn feedback_adjustment(&self) -> Option<(String, i16)> {
        // Score over DECISIVE runs only: completed + failed. `stopped` =
        // user-cancelled runs (and abandoned permission prompts) are NOT model
        // outcomes, so they are excluded from both the numerator and the
        // denominator — a cancel neither penalizes the model nor dilutes its
        // score. Need >=2 decisive samples before nudging routing.
        let decisive = self.completed.saturating_add(self.failed);
        if decisive < 2 {
            return None;
        }
        let completed = i32::try_from(self.completed).unwrap_or(i32::MAX);
        let failed = i32::try_from(self.failed).unwrap_or(i32::MAX);
        let decisive = i32::try_from(decisive).unwrap_or(i32::MAX).max(1);
        // Confidence-weighted (P3): the win-rate `(completed - failed)/decisive`
        // is scaled to the full feedback bound only once there are enough decisive
        // samples (`CONFIDENT_DECISIVE_SAMPLES`); below that it ramps up linearly,
        // so two lucky runs nudge gently while a well-evidenced model can reach the
        // full `MAX_FEEDBACK_ADJUSTMENT` — large enough to override the recency
        // (`release_rank`) prior within the role's candidate pool, which the old
        // ±40 bound never could. Still bounded under the capability/tier gates.
        let confidence = decisive.min(CONFIDENT_DECISIVE_SAMPLES);
        let max = i32::from(MAX_FEEDBACK_ADJUSTMENT);
        let score = ((completed - failed) * max * confidence)
            / (decisive * CONFIDENT_DECISIVE_SAMPLES);
        Some((
            self.selected_model.clone(),
            i16::try_from(score.clamp(-max, max)).unwrap_or(0),
        ))
    }
}

/// Per-decision-kind outcome tally (P1) — the substrate for "which KIND of
/// orchestration decision is going wrong", split out from the per-model
/// [`RouteOutcomeBucket`] so the accuracy dashboard (and a later learning
/// phase) can read the decision axis directly. Decisive counting reuses the
/// SAME `decisive_outcome` classification the per-model buckets use, so a
/// throttled-provider fault never counts against a decision kind either.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionOutcomeStat {
    /// Canonical decision label (a [`DecisionKind::as_str`], or a raw foreign
    /// label an older/newer writer used).
    pub decision: String,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    /// Records excluded from the decisive pool (user-cancelled `stopped`, or an
    /// infra-class provider fault) — kept for the dashboard's denominator.
    pub nondecisive: usize,
    /// Records marked `reworked` — the work landed but needed a follow-up fix.
    pub reworked: usize,
    /// Records a human `intervened` on.
    pub intervened: usize,
}

impl DecisionOutcomeStat {
    /// Decisive first-try success rate `completed / (completed + failed)` in
    /// `0.0..=1.0`, or `None` when the kind has no decisive samples yet.
    #[must_use]
    pub fn success_rate(&self) -> Option<f64> {
        rate(self.completed, self.decisive())
    }

    #[must_use]
    fn decisive(&self) -> usize {
        self.completed.saturating_add(self.failed)
    }
}

/// Tally every record by its [`DecisionKind`] label (an absent label counts as
/// `model`, the historical default), returning one [`DecisionOutcomeStat`] per
/// distinct label seen, in canonical [`DecisionKind::ALL`] order first, then
/// any foreign labels alphabetically. Pure — the aggregation half of P1, with
/// no I/O, exercised directly by the tests.
#[must_use]
pub fn summarize_decisions_by_kind(records: &[RouteOutcomeRecord]) -> Vec<DecisionOutcomeStat> {
    let mut by_kind: BTreeMap<String, DecisionOutcomeStat> = BTreeMap::new();
    for record in records {
        // An explicit label (even a foreign one) keeps its raw string; an
        // absent one takes the inferred kind (legacy verdict -> verify).
        let label = record
            .decision
            .clone()
            .unwrap_or_else(|| record.decision_kind().as_str().to_string());
        let stat = by_kind
            .entry(label.clone())
            .or_insert_with(|| DecisionOutcomeStat {
                decision: label,
                ..DecisionOutcomeStat::default()
            });
        stat.total = stat.total.saturating_add(1);
        match decisive_outcome(record.status.as_str(), record.provider_error_class.as_deref()) {
            Some(true) => stat.completed = stat.completed.saturating_add(1),
            Some(false) => stat.failed = stat.failed.saturating_add(1),
            None => stat.nondecisive = stat.nondecisive.saturating_add(1),
        }
        if record.reworked == Some(true) {
            stat.reworked = stat.reworked.saturating_add(1);
        }
        if record.intervened == Some(true) {
            stat.intervened = stat.intervened.saturating_add(1);
        }
    }
    let mut stats: Vec<DecisionOutcomeStat> = by_kind.into_values().collect();
    stats.sort_by(|left, right| {
        decision_label_order(&left.decision).cmp(&decision_label_order(&right.decision))
    });
    stats
}

/// Sort key placing the known [`DecisionKind::ALL`] kinds first in canonical
/// order, then any foreign label alphabetically after them.
fn decision_label_order(label: &str) -> (usize, String) {
    DecisionKind::ALL
        .iter()
        .position(|kind| kind.as_str() == label)
        .map_or_else(
            || (DecisionKind::ALL.len(), label.to_string()),
            |index| (index, String::new()),
        )
}

/// The decision kind with the WORST confidence-weighted margin
/// (`confidence_weighted_margin`) among those with at least `min_decisive`
/// decisive samples — the "where is orchestration going wrong" answer P1
/// produces for the dashboard. Ranking on the weighted margin (not a raw rate)
/// means ninety evidenced failures outrank two thin ones. `None` when no kind
/// clears the sample floor. Ties break toward more decisive evidence, then the
/// canonical label order, so the result is deterministic. `min_decisive` is a
/// caller-injected floor (no hidden constant): thin, noisy kinds stay out
/// until they have real evidence.
#[must_use]
pub fn weakest_decision_kind(
    stats: &[DecisionOutcomeStat],
    min_decisive: usize,
) -> Option<&DecisionOutcomeStat> {
    stats
        .iter()
        .filter(|stat| stat.decisive() >= min_decisive.max(1))
        .min_by(|left, right| {
            // A kind past the floor always has decisive samples, so the
            // fallback only guards the type — it never decides an ordering.
            let left_score = confidence_weighted_margin(left.completed, left.failed).unwrap_or(1.0);
            let right_score =
                confidence_weighted_margin(right.completed, right.failed).unwrap_or(1.0);
            left_score
                .partial_cmp(&right_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.decisive().cmp(&left.decisive()))
                .then_with(|| {
                    decision_label_order(&left.decision).cmp(&decision_label_order(&right.decision))
                })
        })
}

/// Verification-quality metrics over the `verify` decision records (P2) — the
/// dashboard's "catch rate" and verdict-basis breakdown. `caught` counts verify
/// decisions that FAILED: a validator that found a real defect, which is the
/// whole point of verifying. `model_only`/`objective` split the verify records
/// by what settled them, so the dashboard can show how much of the verify
/// signal is only an LLM opinion (see the design's honest-risk section).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerifyMetrics {
    pub total: usize,
    /// Verify decisions that FAILED about the WORK — the validator found a
    /// real defect. A validator whose own output was unusable records a failed
    /// verify against itself (`verdict_subject: validator`) and is counted in
    /// `validator_faults` instead, never here.
    pub caught: usize,
    /// Failed verifies that were the validator's own fault (unusable output),
    /// not a defect in the work.
    pub validator_faults: usize,
    pub model_only: usize,
    pub objective: usize,
}

impl VerifyMetrics {
    /// Fraction of verify decisions that caught a defect, `0.0..=1.0`, or
    /// `None` when nothing has been verified yet.
    #[must_use]
    pub fn catch_rate(&self) -> Option<f64> {
        rate(self.caught, self.total)
    }

    /// Fraction of verify decisions settled by model judgement alone (no
    /// objective signal) — the honest-risk gauge, `None` when none verified.
    #[must_use]
    pub fn model_only_rate(&self) -> Option<f64> {
        rate(self.model_only, self.total)
    }
}

/// Tally the `verify` decision records (P2). A failed verify is a defect the
/// validator caught; an infra-class provider fault is neither a catch nor a
/// pass (reuses `decisive_outcome`, the same classifier the per-model buckets
/// use), so a throttled verify run never inflates the catch rate.
#[must_use]
pub fn verify_metrics(records: &[RouteOutcomeRecord]) -> VerifyMetrics {
    let mut metrics = VerifyMetrics::default();
    for record in records {
        if record.decision_kind() != DecisionKind::Verify {
            continue;
        }
        metrics.total = metrics.total.saturating_add(1);
        if decisive_outcome(record.status.as_str(), record.provider_error_class.as_deref())
            == Some(false)
        {
            match record.verdict_subject_kind() {
                VerdictSubject::Work => metrics.caught = metrics.caught.saturating_add(1),
                VerdictSubject::Validator => {
                    metrics.validator_faults = metrics.validator_faults.saturating_add(1);
                }
            }
        }
        match record.verdict_basis_kind() {
            VerdictBasis::Objective => metrics.objective = metrics.objective.saturating_add(1),
            VerdictBasis::Model => metrics.model_only = metrics.model_only.saturating_add(1),
        }
    }
    metrics
}

#[must_use]
pub fn route_outcome_log_path(cwd: &Path) -> PathBuf {
    crate::zo_project_state_dir(cwd).join(OUTCOME_DIR).join(OUTCOME_FILE)
}

/// Statuses a route outcome may actually persist. A route outcome represents
/// a FINISHED run; `still_running` (and any other in-flight/placeholder
/// label) is a live-progress signal only — see [`record_route_outcome`]'s
/// doctrine guard.
#[must_use]
pub fn is_terminal_outcome_status(status: &str) -> bool {
    matches!(
        status,
        OUTCOME_COMPLETED | OUTCOME_FAILED | OUTCOME_STOPPED
    )
}

/// The run finished on its own terms.
pub const OUTCOME_COMPLETED: &str = "completed";
/// The run ended in an error.
pub const OUTCOME_FAILED: &str = "failed";
/// Something cut the run short — a wall clock, a cancel — so nothing was
/// learned about whether it would have succeeded.
pub const OUTCOME_STOPPED: &str = "stopped";

/// Recorder-side doctrine guard (shared by every recorder — the spawn
/// completion path, verdict attribution, and any future source): a route
/// outcome record is only ever appended for a TERMINAL status
/// (`completed`/`failed`/`stopped`). `still_running` is a live/in-flight
/// placeholder (HUD, collection-window stragglers) — persisting it would
/// silently poison the decisive aggregate as a false failure the moment it
/// landed (see `normalized_status`/`add_status_to_bucket`).
///
/// `debug_assert!` means a violation PANICS in development/test builds (the
/// bug is caught immediately, loudly, at the call site that introduced it)
/// but the write is unconditionally skipped either way, so a release build
/// degrades to "one record silently dropped" instead of ever crashing on it.
pub fn record_route_outcome(cwd: &Path, record: &RouteOutcomeRecord) -> io::Result<()> {
    debug_assert!(
        is_terminal_outcome_status(&record.status),
        "route-outcome recorder doctrine violation: attempted to persist non-terminal status {:?} for route_key {:?} — every recorder must record only completed/failed/stopped",
        record.status,
        record.route_key,
    );
    if !is_terminal_outcome_status(&record.status) {
        return Ok(());
    }
    record_route_outcome_at_path(&route_outcome_log_path(cwd), record)
}

fn record_route_outcome_at_path(path: &Path, record: &RouteOutcomeRecord) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Serialize the whole line first and land it in ONE write: an O_APPEND
    // write of a single buffer is atomic against other appenders on a regular
    // file, whereas streaming serde_json straight into the File issued many
    // small write()s that two concurrent recorders zippered byte-for-byte —
    // 5 such lines across 3,912 live records at re-verification, each collision
    // destroying BOTH records (`concurrent_recorders_never_interleave_a_line`
    // reproduced it as 13 lost lines in 320). One buffer, one write.
    let mut line = serde_json::to_string(record).map_err(io::Error::other)?;
    line.push('\n');
    {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        file.write_all(line.as_bytes())?;
    }
    prune_route_outcome_store(path)
}

/// P3 retention v2: per-bucket cap + global cap (see the module-level consts)
/// in place of the old flat line-count cap. Operates on the file's OWN lines
/// via [`rewrite_jsonl_lines_if_changed`] (symlink-safe, atomic rename) so a
/// retained record's on-disk bytes are untouched — only dropped lines change.
fn prune_route_outcome_store(path: &Path) -> io::Result<()> {
    rewrite_jsonl_lines_if_changed(path, |lines| {
        let parsed: Option<Vec<RouteOutcomeRecord>> = lines
            .iter()
            .map(|line| serde_json::from_str(line).ok())
            .collect();
        let Some(records) = parsed else {
            // A line this build cannot parse (corrupt write, or a future/
            // foreign schema) cannot be bucketed safely — fall back to the
            // old flat newest-cap on raw lines so the file still stays
            // bounded instead of growing without limit.
            return flat_cap_newest_lines(lines, OUTCOME_GLOBAL_RETENTION);
        };
        let keep = outcome_prune_keep_mask(&records);
        lines
            .into_iter()
            .zip(keep)
            .filter_map(|(line, keep)| keep.then_some(line))
            .collect()
    })
}

fn flat_cap_newest_lines(lines: Vec<String>, cap: usize) -> Vec<String> {
    if lines.len() <= cap {
        return lines;
    }
    let skip = lines.len() - cap;
    lines.into_iter().skip(skip).collect()
}

/// Which of `records` (append/file order — oldest first) survive retention.
/// Two passes:
/// 1. Per-`(route_key, selectedModel)` bucket cap: a single bucket keeps only
///    its newest [`OUTCOME_BUCKET_RETENTION`] records.
/// 2. Global cap: if bucket-capped survivors still exceed
///    [`OUTCOME_GLOBAL_RETENTION`], evict the globally-oldest surviving
///    record from whichever bucket is CURRENTLY LARGEST, repeated until back
///    under budget — protecting a low-traffic route's thin history until the
///    very end.
fn outcome_prune_keep_mask(records: &[RouteOutcomeRecord]) -> Vec<bool> {
    let mut keep = vec![true; records.len()];
    let mut order: BTreeMap<(String, String), VecDeque<usize>> = BTreeMap::new();
    for (idx, record) in records.iter().enumerate() {
        order
            .entry((record.route_key.clone(), record.selected_model.clone()))
            .or_default()
            .push_back(idx);
    }
    for indices in order.values_mut() {
        while indices.len() > OUTCOME_BUCKET_RETENTION {
            if let Some(oldest) = indices.pop_front() {
                keep[oldest] = false;
            }
        }
    }
    let mut survivors: usize = order.values().map(VecDeque::len).sum();
    if survivors > OUTCOME_GLOBAL_RETENTION {
        let mut heap: BinaryHeap<(usize, (String, String))> = order
            .iter()
            .filter(|(_, indices)| !indices.is_empty())
            .map(|(key, indices)| (indices.len(), key.clone()))
            .collect();
        while survivors > OUTCOME_GLOBAL_RETENTION {
            let Some((size, key)) = heap.pop() else { break };
            let Some(indices) = order.get_mut(&key) else { continue };
            if indices.len() != size {
                // Stale heap entry (this bucket's size already changed since
                // it was pushed) — push the current size back and retry.
                if !indices.is_empty() {
                    heap.push((indices.len(), key));
                }
                continue;
            }
            if let Some(oldest) = indices.pop_front() {
                keep[oldest] = false;
                survivors -= 1;
                if !indices.is_empty() {
                    heap.push((indices.len(), key));
                }
            }
        }
    }
    keep
}

/// Pure record-level pruning (the same policy as [`prune_route_outcome_store`],
/// without the file I/O) — exercised directly by the retention tests below,
/// which is cheaper and more precise than round-tripping through real files.
#[cfg(test)]
#[must_use]
fn prune_outcome_records(records: Vec<RouteOutcomeRecord>) -> Vec<RouteOutcomeRecord> {
    let keep = outcome_prune_keep_mask(&records);
    records
        .into_iter()
        .zip(keep)
        .filter_map(|(record, keep)| keep.then_some(record))
        .collect()
}

pub fn read_route_outcomes(cwd: &Path) -> io::Result<Vec<RouteOutcomeRecord>> {
    read_route_outcomes_from_path(&route_outcome_log_path(cwd))
}

pub(super) fn read_route_outcomes_from_path(path: &Path) -> io::Result<Vec<RouteOutcomeRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = OpenOptions::new().read(true).open(path)?;
    Ok(BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<RouteOutcomeRecord>(&line).ok())
        .collect())
}

pub fn read_route_outcome_summary(cwd: &Path) -> io::Result<RouteOutcomeSummary> {
    Ok(summarize_route_outcomes(&read_route_outcomes(cwd)?))
}

#[must_use]
pub fn summarize_route_outcomes(records: &[RouteOutcomeRecord]) -> RouteOutcomeSummary {
    summarize_route_outcomes_with_canonicalizer(records, ToString::to_string)
}

/// Same aggregation as [`summarize_route_outcomes`], but `canonicalize_model`
/// is applied to each record's `selected_model` BEFORE it becomes (part of)
/// a bucket key — so historical id fragments that name the same model
/// (`claude-opus-4-8` vs `claude-opus-4.8`, `fable`/`fable5` vs
/// `claude-fable-5`, `gpt-5.5` vs its dated canonical id) merge into one
/// bucket instead of diluting the decisive sample count across N near-
/// duplicate buckets.
///
/// This is the pure engine's half of P3 canonicalization — it stays free of
/// any alias-resolution logic itself (`crate::model_router` has no
/// dependency on `api`); the tools layer builds the actual canonicalizer
/// (on `api::resolve_model_alias`) and injects it here. [`summarize_route_outcomes`]
/// passes the identity closure, so every EXISTING caller keeps its current,
/// byte-identical behavior; only a caller that opts into this variant sees
/// merged buckets.
///
/// Both aggregate the router's LEARNING samples only (`learning_sample_mask`):
/// one sample per attributed attempt — its settled verdict when it has one —
/// and never a fold. Unattributed records (every legacy line) all count.
#[must_use]
pub fn summarize_route_outcomes_with_canonicalizer(
    records: &[RouteOutcomeRecord],
    canonicalize_model: impl Fn(&str) -> String,
) -> RouteOutcomeSummary {
    let mut summary = RouteOutcomeSummary::default();
    let mut buckets: BTreeMap<(String, String, String, String), RouteOutcomeBucket> = BTreeMap::new();
    for record in learning_samples(records) {
        summary.total = summary.total.saturating_add(1);
        summary.output_tokens = summary.output_tokens.saturating_add(record.output_tokens);
        add_status_to_summary(&mut summary, record.status.as_str());
        let canonical_model = canonicalize_model(&record.selected_model);
        let key = (
            record.route_key.clone(),
            record.target_kind.clone(),
            record.target.clone(),
            canonical_model.clone(),
        );
        let bucket = buckets.entry(key).or_insert_with(|| RouteOutcomeBucket {
            route_key: record.route_key.clone(),
            target_kind: record.target_kind.clone(),
            target: record.target.clone(),
            selected_model: canonical_model,
            total: 0,
            completed: 0,
            failed: 0,
            stopped: 0,
            output_tokens: 0,
            output_token_samples: 0,
            provider_errors: BTreeMap::new(),
        });
        bucket.total = bucket.total.saturating_add(1);
        bucket.output_tokens = bucket.output_tokens.saturating_add(record.output_tokens);
        if record.output_tokens > 0 {
            bucket.output_token_samples = bucket.output_token_samples.saturating_add(1);
        }
        add_status_to_bucket(
            bucket,
            record.status.as_str(),
            record.provider_error_class.as_deref(),
        );
        if let Some(class) = record.provider_error_class.as_deref() {
            *bucket.provider_errors.entry(class.to_string()).or_insert(0) += 1;
        }
    }
    summary.by_route = buckets.into_values().collect();
    summary.by_route.sort_by(|left, right| {
        right
            .total
            .cmp(&left.total)
            .then_with(|| left.route_key.cmp(&right.route_key))
    });
    summary
}

fn add_status_to_summary(summary: &mut RouteOutcomeSummary, status: &str) {
    match normalized_status(status) {
        OutcomeStatus::Completed => summary.completed = summary.completed.saturating_add(1),
        OutcomeStatus::Failed => summary.failed = summary.failed.saturating_add(1),
        OutcomeStatus::Stopped => summary.stopped = summary.stopped.saturating_add(1),
        OutcomeStatus::StillRunning => {
            summary.still_running = summary.still_running.saturating_add(1);
        }
    }
}

fn add_status_to_bucket(
    bucket: &mut RouteOutcomeBucket,
    status: &str,
    provider_error_class: Option<&str>,
) {
    match normalized_status(status) {
        OutcomeStatus::Completed => bucket.completed = bucket.completed.saturating_add(1),
        OutcomeStatus::Stopped => bucket.stopped = bucket.stopped.saturating_add(1),
        OutcomeStatus::Failed | OutcomeStatus::StillRunning => {
            // Provider-infrastructure failures (throttling, transient faults,
            // expired credentials) are not model-quality outcomes: a throttled
            // provider's model must not lose win-rate for non-quality reasons.
            // They stay in `total` and `provider_errors` (dashboard) but are
            // excluded from the decisive denominator, like `stopped`. Model
            // faults (contextOverflow / invalid tool protocol / safetyBlocked /
            // nonRetryable) still count as failures.
            if is_infra_provider_error(provider_error_class) {
                return;
            }
            bucket.failed = bucket.failed.saturating_add(1);
        }
    }
}

/// Shared with `decisive_outcome` so there is exactly one place that
/// decides which provider-error classes are infrastructure noise (never a
/// model-quality signal), instead of two independently-maintained copies of
/// the same literal set.
fn is_infra_provider_error(provider_error_class: Option<&str>) -> bool {
    // `providerOverloaded` is the capacity label a sub-agent reports when the
    // PROVIDER shed the request (429's sibling — see `api::CapacityScope`). It is
    // infrastructure noise for exactly the same reason `rateLimit` is: the model
    // never got to answer, so counting it as a quality loss would demote whichever
    // model happened to be routed during a capacity dip.
    matches!(
        provider_error_class,
        Some("rateLimit" | "providerOverloaded" | "transient" | "authExpired")
    )
}

/// Whether a record is a decisive win (`Some(true)`), a decisive loss
/// (`Some(false)`), or excluded from the decisive pool entirely (`None`) —
/// `stopped` (user-cancelled) and an infra-class failure never count either
/// way. Reuses [`normalized_status`]/[`is_infra_provider_error`] so this is
/// the SAME classification [`add_status_to_bucket`] applies, just projected
/// to a signed outcome instead of an aggregate mutation — used by
/// [`weighted_feedback_hint_for_route_key`], which needs a per-record verdict
/// (not a pre-aggregated bucket count) to apply recency weighting.
pub(super) fn decisive_outcome(status: &str, provider_error_class: Option<&str>) -> Option<bool> {
    match normalized_status(status) {
        OutcomeStatus::Completed => Some(true),
        OutcomeStatus::Stopped => None,
        OutcomeStatus::Failed | OutcomeStatus::StillRunning => {
            if is_infra_provider_error(provider_error_class) {
                None
            } else {
                Some(false)
            }
        }
    }
}

/// Which of `records` (store order — oldest first) the router LEARNS from,
/// index-aligned. The store holds more than one receipt per attempt: the
/// spawn recorder's run outcome ("the model finished"), then any verdicts
/// about that same attempt ("the work was right"), and for a lane the union
/// dropped, a fold. Counting each as its own sample let a finished attempt
/// whose work FAILED verification net zero (one win, one loss), let a
/// validator whose output was unusable net zero on its own attempt, counted a
/// restated verdict twice, and turned every wasted lane into a second win.
///
/// The rule, one place for every learner (the per-route summary, the weighted
/// hint, learned specialty, agent routing, complexity calibration):
/// - a `fold` never teaches: it is the accuracy report's wasted-spawn
///   accounting, and the lane's run already has its own receipt;
/// - an attributed attempt (`run_id`) is ONE sample. A settled verdict about
///   it outranks its run outcome; once one verdict has caught a failure a
///   later pass does not clear it (the attempt was right only if every check
///   that judged it passed); a verification that ended WITHOUT a verdict
///   leaves the attempt unverified — its completion no longer counts as a
///   win, and nothing counts against the work; otherwise the later receipt
///   speaks;
/// - an unattributed record (every line before `run_id`, a main-turn
///   verdict) is its own sample, exactly as before.
///
/// The accuracy report reads the raw decisions instead — it counts what the
/// orchestration DID, this counts what each attempt proved.
pub(super) fn learning_sample_mask(records: &[RouteOutcomeRecord]) -> Vec<bool> {
    let mut speaker: HashMap<&str, usize> = HashMap::new();
    for (index, record) in records.iter().enumerate() {
        if record.decision_kind().is_bookkeeping() {
            continue;
        }
        let Some(attempt) = record.run_id.as_deref() else {
            continue;
        };
        match speaker.entry(attempt) {
            Entry::Vacant(slot) => {
                slot.insert(index);
            }
            Entry::Occupied(mut slot) => {
                if speaks_over(record, &records[*slot.get()]) {
                    *slot.get_mut() = index;
                }
            }
        }
    }
    records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            !record.decision_kind().is_bookkeeping()
                && record
                    .run_id
                    .as_deref()
                    .is_none_or(|attempt| speaker.get(attempt) == Some(&index))
        })
        .collect()
}

/// Selected evidence uses the model that actually ended the SAME attempt.
/// A reviewer can be bound before a runtime swap; the immutable run receipt
/// supplies that final model without reading a newer generation's manifest.
/// Historical receipts themselves remain untouched.
pub(super) fn learning_samples(
    records: &[RouteOutcomeRecord],
) -> impl Iterator<Item = Cow<'_, RouteOutcomeRecord>> {
    let mask = learning_sample_mask(records);
    let models: HashMap<&str, &str> = records.iter().filter_map(|record| {
        (record.decision_kind() == DecisionKind::Model && record.signal.is_none()
            && is_terminal_outcome_status(&record.status))
            .then(|| record.run_id.as_deref().map(|id| (id, record.selected_model.as_str())))
            .flatten()
    }).collect();
    records.iter().zip(mask).filter_map(move |(record, learns)| {
        if !learns { return None; }
        if record.decision_kind() == DecisionKind::Verify {
            if let Some(model) = record.run_id.as_deref().and_then(|id| models.get(id)) {
                if *model != record.selected_model {
                    let mut attributed = record.clone();
                    attributed.selected_model = (*model).to_string();
                    return Some(Cow::Owned(attributed));
                }
            }
        }
        Some(Cow::Borrowed(record))
    })
}

/// Whether `later` (appended after `held`, both about one attempt) speaks for
/// that attempt instead of `held`. A settled verdict outranks a verification
/// that settled nothing, which outranks the run outcome; between settled
/// verdicts a pass never displaces a caught failure; anything else is a
/// newer word of the same standing.
fn speaks_over(later: &RouteOutcomeRecord, held: &RouteOutcomeRecord) -> bool {
    match standing(later).cmp(&standing(held)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => !matches!(
            (settled_verdict(later), settled_verdict(held)),
            (Some(true), Some(false))
        ),
    }
}

/// What a receipt can say about its attempt: `2` a settled verdict ("the work
/// was right / wrong"), `1` a verification that ended without one (unusable
/// output, a verifier that never finished — the attempt is UNVERIFIED, which
/// is not evidence its work failed: non-decisive, but it withdraws the bare
/// completion credit), `0` anything else (the run outcome).
fn standing(record: &RouteOutcomeRecord) -> u8 {
    match (record.decision_kind(), settled_verdict(record)) {
        (DecisionKind::Verify, Some(_)) => 2,
        (DecisionKind::Verify, None) => 1,
        _ => 0,
    }
}

/// `Some(passed)` for a verify record that settled pass/fail, `None` for
/// anything else (a run outcome, or a verify that settled nothing).
fn settled_verdict(record: &RouteOutcomeRecord) -> Option<bool> {
    if record.decision_kind() != DecisionKind::Verify {
        return None;
    }
    decisive_outcome(record.status.as_str(), record.provider_error_class.as_deref())
}

/// Recency-half-life-weighted analogue of
/// [`RouteOutcomeSummary::feedback_hint_for_route_key`]: each decisive
/// (`completed`/`failed`) record's contribution to the confidence-weighted
/// adjustment is scaled by `0.5^(age_days / FEEDBACK_HALF_LIFE_DAYS)` before
/// the win-rate/confidence-ramp math runs, so a model with only aging
/// evidence decays back toward a neutral (0) adjustment instead of staying
/// pinned at the full bound forever (the plain-count aggregate in
/// `RouteOutcomeBucket::feedback_adjustment` has no notion of "when" — a
/// bucket frozen at its `CONFIDENT_DECISIVE_SAMPLES` ramp stays at the same
/// score whether its evidence is an hour or a year old).
///
/// `now` is INJECTED (epoch seconds) — no hidden clock read — so this stays a
/// pure, deterministically-testable function; callers pass live epoch
/// seconds, tests pass fixed values. `canonicalize_model` mirrors
/// [`summarize_route_outcomes_with_canonicalizer`]'s seam so weighted buckets
/// merge the same historical id fragments the unweighted summary does.
///
/// NOT wired into the live routing path for this phase: `apply.rs`'s
/// `SmartRouteContext::feedback_for` still calls the unweighted
/// [`RouteOutcomeSummary::feedback_hint_for_route_key`], which is the ONLY
/// routing-affecting feedback source today — P3 is schema/substrate-only
/// (routing behavior stays byte-identical); a later phase can switch the
/// live call site to this function once that behavior change is explicitly
/// signed off.
///
/// **Still unwired as of Phase 6** (`model_router::learned`): Phase 6 reuses
/// this fn's underlying MACHINERY — `recency_weight` and `decisive_outcome`
/// are shared, `pub(super)`, with the learned-specialty aggregator — but does
/// NOT call `weighted_feedback_hint_for_route_key` itself, and does not flip
/// `apply.rs`'s plain per-route-key feedback path onto it either. That
/// remains a deliberately separate, deferred behavior-freeze decision until
/// the sibling path is explicitly approved after soak.
#[must_use]
pub fn weighted_feedback_hint_for_route_key(
    records: &[RouteOutcomeRecord],
    route_key: &str,
    now: u64,
    canonicalize_model: impl Fn(&str) -> String,
) -> RouteFeedbackHint {
    let mut per_model: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for record in learning_samples(records) {
        if record.route_key != route_key {
            continue;
        }
        let Some(win) = decisive_outcome(record.status.as_str(), record.provider_error_class.as_deref()) else {
            continue;
        };
        let weight = recency_weight(record.recorded_at, now);
        let entry = per_model
            .entry(canonicalize_model(&record.selected_model))
            .or_insert((0.0, 0.0));
        if win {
            entry.0 += weight;
        } else {
            entry.1 += weight;
        }
    }
    per_model
        .into_iter()
        .fold(RouteFeedbackHint::disabled(), |hint, (model, (weighted_completed, weighted_failed))| {
            match weighted_feedback_adjustment(weighted_completed, weighted_failed) {
                Some(adjustment) => hint.with_model_adjustment(model, adjustment),
                None => hint,
            }
        })
}

/// `0.5^(age_days / FEEDBACK_HALF_LIFE_DAYS)`, clamped implicitly to `(0, 1]`
/// by construction (`age_seconds` saturates at 0 for a `recorded_at` at or
/// after `now`, e.g. a fixed test `now` or minor clock skew — that reads as
/// "fresh", weight 1.0, never > 1.0 or negative).
#[allow(clippy::cast_precision_loss)]
pub(super) fn recency_weight(recorded_at: u64, now: u64) -> f64 {
    let age_seconds = now.saturating_sub(recorded_at);
    let age_days = age_seconds as f64 / 86_400.0;
    0.5_f64.powf(age_days / FEEDBACK_HALF_LIFE_DAYS)
}

/// Same confidence-ramp shape as [`RouteOutcomeBucket::feedback_adjustment`]
/// (see its doc for the rationale), generalized from integer decisive counts
/// to recency-weighted `f64` sums. At weight 1.0 per record (i.e. every
/// record dated exactly `now`) this produces the IDENTICAL score the integer
/// version would for the same raw completed/failed counts — the ramp and
/// bound math are the same formula, just over weighted sums.
fn weighted_feedback_adjustment(weighted_completed: f64, weighted_failed: f64) -> Option<i16> {
    let decisive = weighted_completed + weighted_failed;
    if decisive < 2.0 {
        return None;
    }
    let confidence = decisive.min(f64::from(CONFIDENT_DECISIVE_SAMPLES));
    let max = f64::from(MAX_FEEDBACK_ADJUSTMENT);
    let score = (weighted_completed - weighted_failed) * max * confidence / (decisive * f64::from(CONFIDENT_DECISIVE_SAMPLES));
    #[allow(clippy::cast_possible_truncation)]
    let bounded = score.round().clamp(-max, max) as i16;
    Some(bounded)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeStatus {
    Completed,
    Failed,
    Stopped,
    StillRunning,
}

fn normalized_status(status: &str) -> OutcomeStatus {
    match status {
        "completed" => OutcomeStatus::Completed,
        "stopped" => OutcomeStatus::Stopped,
        "still_running" => OutcomeStatus::StillRunning,
        _ => OutcomeStatus::Failed,
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// `num / den` as a rate in `0.0..=1.0`, or `None` when `den == 0`. The ONE
/// zero-guarded division every rate in this module and its siblings
/// (`agent_route`, `accuracy`) uses, so "no evidence" reads as `None`
/// everywhere instead of a fabricated `0.0` — and the guard lives in one place
/// rather than five copies (re-verification finding, 2026-09-10).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(super) fn rate(num: usize, den: usize) -> Option<f64> {
    if den == 0 {
        None
    } else {
        Some(num as f64 / den as f64)
    }
}

/// Confidence-weighted win margin in `-1.0..=1.0`: `(completed - failed) /
/// decisive`, scaled by `min(decisive, CONFIDENT_DECISIVE_SAMPLES) /
/// CONFIDENT_DECISIVE_SAMPLES` — the SAME ramp
/// [`RouteOutcomeBucket::feedback_adjustment`] applies, so two lucky runs score
/// gently while a well-evidenced record can reach the full bound. Every ranker
/// in this module and its siblings orders on THIS, never on a raw rate: a raw
/// rate at the sample floor lets a 2-of-2 outrank a 95-of-100 and a 0-of-2
/// outrank a 10-of-100 (re-verification finding, 2026-09-10). `None` when
/// there are no decisive samples.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(super) fn confidence_weighted_margin(completed: usize, failed: usize) -> Option<f64> {
    let decisive = completed.saturating_add(failed);
    if decisive == 0 {
        return None;
    }
    let confident = usize::try_from(CONFIDENT_DECISIVE_SAMPLES).unwrap_or(1).max(1);
    let margin = (completed as f64 - failed as f64) / decisive as f64;
    let confidence = decisive.min(confident) as f64 / confident as f64;
    Some(margin * confidence)
}

/// The legacy provenance label a verdict recorder stamps in `signal` (the
/// cross-check attribution and the ad-hoc turn review both write it). Records
/// from before the decision axis carry ONLY this — the reader infers
/// [`DecisionKind::Verify`] from it (see `RouteOutcomeRecord::decision_kind`).
pub const VERDICT_SIGNAL: &str = "verdict";

fn epoch_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn summary_counts_statuses_without_raw_outputs() {
        let records = vec![
            RouteOutcomeRecord::new("subagent", "Verification", "model-a", "completed")
                .with_output_tokens(7),
            RouteOutcomeRecord::new("subagent", "Verification", "model-a", "failed")
                .with_provider_error_class(Some("rateLimit".to_string())),
            RouteOutcomeRecord::new("subagent", "debugger", "model-b", "stopped"),
        ];

        let summary = summarize_route_outcomes(&records);

        assert_eq!(summary.total, 3);
        assert_eq!(summary.completed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.stopped, 1);
        assert_eq!(summary.output_tokens, 7);
        assert_eq!(summary.by_route[0].route_key, "subagent:Verification");
        assert_eq!(summary.by_route[0].provider_errors["rateLimit"], 1);
    }

    #[test]
    fn decisive_counts_and_total_records_are_scoped_to_the_route_key() {
        let records = vec![
            RouteOutcomeRecord::new("subagent", "code-reviewer", "model-hot", "completed"),
            RouteOutcomeRecord::new("subagent", "code-reviewer", "model-hot", "completed"),
            RouteOutcomeRecord::new("subagent", "code-reviewer", "model-hot", "failed"),
            RouteOutcomeRecord::new("subagent", "code-reviewer", "gpt-5.6-sol", "completed"),
            // A cancelled run: counts toward `total` but not decisive.
            RouteOutcomeRecord::new("subagent", "code-reviewer", "gpt-5.6-sol", "stopped"),
            // A different route_key entirely must not leak in.
            RouteOutcomeRecord::new("subagent", "debugger", "model-hot", "completed"),
        ];

        let summary = summarize_route_outcomes(&records);
        let route_key = "subagent:code-reviewer";

        let mut counts = summary.decisive_counts_for_route_key(route_key);
        counts.sort();
        assert_eq!(
            counts,
            vec![("gpt-5.6-sol".to_string(), 1), ("model-hot".to_string(), 3)]
        );
        assert_eq!(summary.total_records_for_route_key(route_key), 5);
        assert_eq!(summary.total_records_for_route_key("subagent:debugger"), 1);
        assert_eq!(summary.decisive_counts_for_route_key("subagent:unknown"), Vec::new());
    }

    #[test]
    fn provider_infra_failures_do_not_count_against_model_quality() {
        // 2 wins + 3 rate-limit failures: infra faults must not turn a good
        // model's feedback negative. They stay visible (total/provider_errors)
        // but leave the decisive pool, so the bucket scores as 2-0.
        let mut records = vec![
            RouteOutcomeRecord::new("subagent", "Verification", "model-a", "completed"),
            RouteOutcomeRecord::new("subagent", "Verification", "model-a", "completed"),
        ];
        for _ in 0..3 {
            records.push(
                RouteOutcomeRecord::new("subagent", "Verification", "model-a", "failed")
                    .with_provider_error_class(Some("rateLimit".to_string())),
            );
        }
        // A model fault (no provider_error_class) still counts as a failure.
        records.push(RouteOutcomeRecord::new(
            "subagent",
            "Verification",
            "model-a",
            "failed",
        ));

        let summary = summarize_route_outcomes(&records);
        let bucket = &summary.by_route[0];

        assert_eq!(bucket.total, 6);
        assert_eq!(bucket.completed, 2);
        assert_eq!(bucket.failed, 1, "only the model fault is decisive");
        assert_eq!(bucket.provider_errors["rateLimit"], 3);
        let hint = summary.feedback_hint_for_route_key("subagent:Verification");
        assert!(
            hint.bounded_adjustment_for("model-a") > 0,
            "2-1 decisive record must stay positive despite 3 throttle faults"
        );
    }

    fn explore_records(model: &str, completed: usize, failed: usize, tokens: u64) -> Vec<RouteOutcomeRecord> {
        let mut records = Vec::new();
        for _ in 0..completed {
            records.push(
                RouteOutcomeRecord::new("subagent", "Explore", model, "completed").with_output_tokens(tokens),
            );
        }
        for _ in 0..failed {
            records.push(
                RouteOutcomeRecord::new("subagent", "Explore", model, "failed").with_output_tokens(tokens),
            );
        }
        records
    }

    /// Same record, dearer tokens: the dearer model pays a bounded tiebreak
    /// (2026-09-10: on this repository two fast models did the same
    /// reconnaissance at 8.0k vs 20.8k median output tokens).
    #[test]
    fn a_dearer_model_with_the_same_record_loses_the_cost_tiebreak() {
        let mut records = explore_records("cheap-fast", 10, 0, 8_000);
        records.extend(explore_records("dear-fast", 10, 0, 20_000));
        let hint = summarize_route_outcomes(&records).feedback_hint_for_route_key("subagent:Explore");
        let cheap = hint.bounded_adjustment_for("cheap-fast");
        let dear = hint.bounded_adjustment_for("dear-fast");
        assert_eq!(cheap, MAX_FEEDBACK_ADJUSTMENT, "the cheapest evidenced model pays nothing");
        // ratio 2.5: (2.5 - 1.25) / (3.0 - 1.25) of the half-bound = 43
        assert_eq!(dear, MAX_FEEDBACK_ADJUSTMENT - 43, "{dear}");
        assert!(dear > 0, "a dearer model that succeeds is still recommended, less");
    }

    #[test]
    fn a_price_within_tolerance_is_the_same_price() {
        let mut records = explore_records("a", 6, 0, 8_000);
        records.extend(explore_records("b", 6, 0, 9_500));
        let hint = summarize_route_outcomes(&records).feedback_hint_for_route_key("subagent:Explore");
        assert_eq!(hint.bounded_adjustment_for("a"), hint.bounded_adjustment_for("b"));
    }

    /// Records without a token figure (the store before 2026-09-10) neither
    /// set the route's reference price nor pay against it.
    #[test]
    fn thin_token_evidence_neither_sets_the_reference_nor_pays() {
        let mut records = explore_records("old", 10, 0, 0);
        records.push(RouteOutcomeRecord::new("subagent", "Explore", "old", "completed").with_output_tokens(500));
        records.extend(explore_records("new", 4, 0, 30_000));
        let summary = summarize_route_outcomes(&records);
        let old = summary.by_route.iter().find(|b| b.selected_model == "old").expect("old bucket");
        assert_eq!(old.output_token_samples, 1);
        assert_eq!(old.mean_output_tokens(), None, "one sample is not a mean");
        let hint = summary.feedback_hint_for_route_key("subagent:Explore");
        // `new` is the only evidenced price, so it is its own reference: no penalty.
        assert_eq!(hint.bounded_adjustment_for("new"), hint.bounded_adjustment_for("new").max(0));
        assert_eq!(
            hint.bounded_adjustment_for("new"),
            summary.by_route.iter().find(|b| b.selected_model == "new").unwrap().feedback_adjustment().unwrap().1
        );
    }

    /// Cost is a tiebreak: a model that fails cheaply never outranks one that
    /// succeeds dearly.
    #[test]
    fn evidence_still_outranks_price() {
        let mut records = explore_records("cheap-failing", 4, 6, 6_000);
        records.extend(explore_records("dear-succeeding", 10, 0, 30_000));
        let hint = summarize_route_outcomes(&records).feedback_hint_for_route_key("subagent:Explore");
        let dear = hint.bounded_adjustment_for("dear-succeeding");
        assert_eq!(dear, MAX_FEEDBACK_ADJUSTMENT - MAX_COST_ADJUSTMENT, "full penalty at 5x the price");
        assert!(dear > hint.bounded_adjustment_for("cheap-failing"), "{dear} vs {}", hint.bounded_adjustment_for("cheap-failing"));
    }

    #[test]
    fn store_round_trips_and_prunes_a_hot_bucket_to_its_cap() {
        // All records share one (route_key, model) bucket, so this exercises
        // the real file-level write→prune→read path against the per-bucket
        // cap (the old test asserted the flat 512 cap; P3 replaces that with
        // a per-bucket cap of `OUTCOME_BUCKET_RETENTION`).
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("route-outcomes.jsonl");
        let extra = 3;
        for index in 0..(OUTCOME_BUCKET_RETENTION + extra) {
            let record = RouteOutcomeRecord::new("subagent", "agent", "model-a", "completed")
                .with_output_tokens(index as u64);
            record_route_outcome_at_path(&path, &record).expect("record outcome");
        }

        let records = read_route_outcomes_from_path(&path).expect("read outcomes");

        assert_eq!(records.len(), OUTCOME_BUCKET_RETENTION);
        // The oldest `extra` records (output_tokens 0..extra) were evicted;
        // the newest `OUTCOME_BUCKET_RETENTION` survive in append order.
        assert_eq!(records[0].output_tokens, extra as u64);
        assert!(!serde_json::to_string(&records[0]).expect("json").contains("prompt"));
    }

    #[test]
    fn bucket_cap_enforced_keeps_newest_records() {
        let mut records = Vec::new();
        for index in 0..60 {
            records.push(
                RouteOutcomeRecord::new("subagent", "agent", "model-a", "completed")
                    .with_output_tokens(index),
            );
        }

        let pruned = prune_outcome_records(records);

        assert_eq!(pruned.len(), OUTCOME_BUCKET_RETENTION);
        // The 12 oldest (0..12) were dropped; 12..60 survive, oldest-first.
        assert_eq!(pruned[0].output_tokens, 12);
        assert_eq!(pruned.last().unwrap().output_tokens, 59);
    }

    #[test]
    fn hot_bucket_burst_cannot_evict_a_quiet_routes_thin_history() {
        // A quiet route with only 3 records must survive a 200-record burst on
        // a totally different (route_key, model) bucket — the live-data
        // problem this retention redesign exists to fix (a hot
        // code-reviewer×gpt-5.5-fast bucket was crowding out thin routes).
        let mut records = Vec::new();
        for index in 0..3 {
            records.push(RouteOutcomeRecord::new(
                "subagent",
                "deep-research",
                "model-quiet",
                "completed",
            ).with_output_tokens(index));
        }
        for index in 0..200 {
            records.push(RouteOutcomeRecord::new(
                "subagent",
                "code-reviewer",
                "model-hot",
                "completed",
            ).with_output_tokens(1000 + index));
        }

        let pruned = prune_outcome_records(records);

        let quiet: Vec<_> = pruned
            .iter()
            .filter(|record| record.selected_model == "model-quiet")
            .collect();
        let hot: Vec<_> = pruned
            .iter()
            .filter(|record| record.selected_model == "model-hot")
            .collect();
        assert_eq!(quiet.len(), 3, "the quiet route's entire history must survive");
        assert_eq!(hot.len(), OUTCOME_BUCKET_RETENTION);
    }

    #[test]
    fn global_cap_evicts_from_the_largest_surviving_bucket_first() {
        // One hot bucket already at the per-bucket cap (48) plus enough
        // singleton (1-record) buckets to push the total one over the global
        // cap. The single excess record must come from the hot bucket, NOT
        // from any of the singleton buckets — that is the whole point of
        // "protect low-traffic route history".
        let mut records = Vec::new();
        for index in 0..OUTCOME_BUCKET_RETENTION {
            records.push(RouteOutcomeRecord::new(
                "subagent",
                "code-reviewer",
                "model-hot",
                "completed",
            ).with_output_tokens(index as u64));
        }
        let singleton_count = OUTCOME_GLOBAL_RETENTION - OUTCOME_BUCKET_RETENTION + 1;
        for index in 0..singleton_count {
            records.push(RouteOutcomeRecord::new(
                "subagent",
                format!("agent-{index}"),
                "model-a",
                "completed",
            ));
        }
        assert_eq!(records.len(), OUTCOME_GLOBAL_RETENTION + 1);

        let pruned = prune_outcome_records(records);

        assert_eq!(pruned.len(), OUTCOME_GLOBAL_RETENTION);
        let hot_survivors = pruned.iter().filter(|r| r.selected_model == "model-hot").count();
        let singleton_survivors = pruned.iter().filter(|r| r.selected_model == "model-a").count();
        assert_eq!(
            hot_survivors,
            OUTCOME_BUCKET_RETENTION - 1,
            "the excess record must come from the largest (hot) bucket"
        );
        assert_eq!(
            singleton_survivors, singleton_count,
            "every low-traffic singleton bucket must survive untouched"
        );
    }

    #[test]
    fn canonicalization_merges_historical_model_id_fragments_at_summarize_time() {
        // Live data showed `claude-opus-4-8` and `claude-opus-4.8` (and
        // similar fragments) diluting the decisive count across two buckets
        // that name the same model. `summarize_route_outcomes` (the identity
        // canonicalizer) keeps them split; the injected canonicalizer merges
        // them into one bucket.
        let records = vec![
            RouteOutcomeRecord::new("subagent", "Plan", "claude-opus-4-8", "completed"),
            RouteOutcomeRecord::new("subagent", "Plan", "claude-opus-4.8", "completed"),
            RouteOutcomeRecord::new("subagent", "Plan", "claude-opus-4.8", "failed"),
        ];

        let unmerged = summarize_route_outcomes(&records);
        assert_eq!(unmerged.by_route.len(), 2, "identity canonicalizer keeps fragments split");

        let merged = summarize_route_outcomes_with_canonicalizer(&records, |model| {
            if model == "claude-opus-4.8" {
                "claude-opus-4-8".to_string()
            } else {
                model.to_string()
            }
        });
        assert_eq!(merged.by_route.len(), 1, "canonicalizer merges the dot/dash fragments");
        let bucket = &merged.by_route[0];
        assert_eq!(bucket.selected_model, "claude-opus-4-8");
        assert_eq!(bucket.total, 3);
        assert_eq!(bucket.completed, 2);
        assert_eq!(bucket.failed, 1);
    }

    #[test]
    fn weighted_feedback_fresh_bucket_still_reaches_full_bound() {
        // 8 decisive, all recorded exactly `now` (weight 1.0 each) must
        // reproduce the SAME full-confidence ±120 the unweighted ramp gives
        // for 8 fresh decisive samples — the half-life axis must not weaken
        // fresh evidence.
        let now = 1_800_000_000_u64;
        let mut records = Vec::new();
        for _ in 0..8 {
            records.push(RouteOutcomeRecord {
                recorded_at: now,
                ..RouteOutcomeRecord::new("subagent", "Verification", "model-a", "completed")
            });
        }

        let hint = weighted_feedback_hint_for_route_key(
            &records,
            "subagent:Verification",
            now,
            ToString::to_string,
        );

        assert_eq!(hint.bounded_adjustment_for("model-a"), MAX_FEEDBACK_ADJUSTMENT);
    }

    #[test]
    fn weighted_feedback_decays_substantially_for_stale_only_evidence() {
        let now = 1_800_000_000_u64;
        let thirty_days_ago = now - 30 * 86_400;
        let mut fresh = Vec::new();
        let mut stale = Vec::new();
        for _ in 0..20 {
            fresh.push(RouteOutcomeRecord {
                recorded_at: now,
                ..RouteOutcomeRecord::new("subagent", "Verification", "model-a", "completed")
            });
            stale.push(RouteOutcomeRecord {
                recorded_at: thirty_days_ago,
                ..RouteOutcomeRecord::new("subagent", "Verification", "model-a", "completed")
            });
        }

        let fresh_hint = weighted_feedback_hint_for_route_key(
            &fresh,
            "subagent:Verification",
            now,
            ToString::to_string,
        );
        let stale_hint = weighted_feedback_hint_for_route_key(
            &stale,
            "subagent:Verification",
            now,
            ToString::to_string,
        );

        assert_eq!(fresh_hint.bounded_adjustment_for("model-a"), MAX_FEEDBACK_ADJUSTMENT);
        let stale_adjustment = stale_hint.bounded_adjustment_for("model-a");
        // 20 decisive samples at 30 days old (weight 0.5^(30/14) ≈ 0.226 each)
        // works out to ≈68, well short of the fresh case's 120 — "substantial"
        // decay without pinning an over-precise hand-computed value.
        assert!(
            stale_adjustment > 0 && stale_adjustment < 100,
            "30-day-old-only evidence must decay substantially below the fresh full bound, got {stale_adjustment}"
        );
    }

    #[test]
    fn weighted_feedback_extremely_stale_thin_evidence_decays_to_disabled() {
        // Only 2 raw decisive samples, both ~90 days old: the weighted
        // decisive sum drops under the `>=2` floor, so the model gets NO
        // adjustment (equivalent to fully decayed to 0) rather than freezing.
        let now = 1_800_000_000_u64;
        let ninety_days_ago = now - 90 * 86_400;
        let records = vec![
            RouteOutcomeRecord {
                recorded_at: ninety_days_ago,
                ..RouteOutcomeRecord::new("subagent", "Verification", "model-a", "completed")
            },
            RouteOutcomeRecord {
                recorded_at: ninety_days_ago,
                ..RouteOutcomeRecord::new("subagent", "Verification", "model-a", "completed")
            },
        ];

        let hint = weighted_feedback_hint_for_route_key(
            &records,
            "subagent:Verification",
            now,
            ToString::to_string,
        );

        assert_eq!(hint.bounded_adjustment_for("model-a"), 0);
    }

    #[test]
    fn weighted_feedback_mixed_ages_weight_each_record_independently() {
        // One very fresh failure should outweigh several old wins once they
        // have decayed enough — this is the "mixed ages weight correctly"
        // acceptance case, asserted against the exact hand-computed score.
        let now = 1_800_000_000_u64;
        let sixty_days_ago = now - 60 * 86_400;
        let mut records = Vec::new();
        for _ in 0..6 {
            records.push(RouteOutcomeRecord {
                recorded_at: sixty_days_ago,
                ..RouteOutcomeRecord::new("subagent", "Verification", "model-a", "completed")
            });
        }
        records.push(RouteOutcomeRecord {
            recorded_at: now,
            ..RouteOutcomeRecord::new("subagent", "Verification", "model-a", "failed")
        });

        let hint = weighted_feedback_hint_for_route_key(
            &records,
            "subagent:Verification",
            now,
            ToString::to_string,
        );

        // weight(60d) = 0.5^(60/14) ≈ 0.0513; weighted_completed = 6 * that
        // ≈ 0.308; weighted_failed = 1.0 (fresh, weight 1.0). decisive ≈ 1.31
        // stays under the `>=2` confidence floor, so the model gets NO
        // adjustment here — vs. the OLD unweighted formula, which would score
        // this bucket +75 (6 completed / 1 failed, confidence-ramped). That
        // gap is "mixed ages weight correctly": the aged wins do not drown out
        // the one fresh loss the way raw counts alone would.
        assert_eq!(hint.bounded_adjustment_for("model-a"), 0);
    }

    #[test]
    fn recency_weight_is_one_at_zero_age_and_decays_monotonically() {
        let now = 1_800_000_000_u64;
        assert!((recency_weight(now, now) - 1.0).abs() < f64::EPSILON);
        // 14 days == `FEEDBACK_HALF_LIFE_DAYS`, spelled as a literal (rather
        // than cast from the `f64` const) to avoid a lossy-cast lint on a
        // whole-number-valued constant.
        debug_assert!((FEEDBACK_HALF_LIFE_DAYS - 14.0).abs() < f64::EPSILON);
        let at_half_life = recency_weight(now - 14 * 86_400, now);
        assert!(
            (at_half_life - 0.5).abs() < 0.001,
            "one half-life must decay to ~0.5, got {at_half_life}"
        );
        assert!(recency_weight(now - 86_400, now) > recency_weight(now - 2 * 86_400, now));
    }

    #[test]
    #[should_panic(expected = "route-outcome recorder doctrine violation")]
    // The doctrine guard is a `debug_assert!`, which compiles out under
    // `--release`; without this gate a release-mode test run fails on "did
    // not panic as expected" even though the guard works as designed.
    #[cfg(debug_assertions)]
    fn record_route_outcome_debug_asserts_on_non_terminal_status() {
        let root = tempfile::tempdir().expect("tempdir");
        let record = RouteOutcomeRecord::new("subagent", "agent", "model-a", "still_running");
        // Must panic in a debug/test build (the doctrine guard) rather than
        // silently writing a `still_running` placeholder to disk.
        let _ = record_route_outcome(root.path(), &record);
    }

    #[test]
    fn is_terminal_outcome_status_accepts_only_finished_states() {
        assert!(is_terminal_outcome_status("completed"));
        assert!(is_terminal_outcome_status("failed"));
        assert!(is_terminal_outcome_status("stopped"));
        assert!(!is_terminal_outcome_status("still_running"));
        assert!(!is_terminal_outcome_status("running"));
    }

    #[test]
    fn v2_fields_round_trip_through_json() {
        let record = RouteOutcomeRecord::new("subagent", "Plan", "gpt-5.6-sol", "completed")
            .with_role(Some("analysis".to_string()))
            .with_complexity(Some("large".to_string()))
            .with_risk(Some("low".to_string()))
            .with_effort_level(Some("ultra".to_string()))
            .with_duration_ms(Some(12_345))
            .with_route_source(Some("auto".to_string()))
            .with_signal_weight(Some(1.5));

        let json = serde_json::to_string(&record).expect("serialize");
        assert!(json.contains("\"role\":\"analysis\""));
        assert!(json.contains("\"complexity\":\"large\""));
        assert!(json.contains("\"risk\":\"low\""));
        assert!(json.contains("\"effortLevel\":\"ultra\""));
        assert!(json.contains("\"durationMs\":12345"));
        assert!(json.contains("\"routeSource\":\"auto\""));
        assert!(json.contains("\"signalWeight\":1.5"));

        let round_tripped: RouteOutcomeRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round_tripped, record);
    }

    /// P1 pair-attribution: a verdict record carrying `verifier_model`
    /// serializes as `verifierModel` and round-trips; a record WITHOUT one
    /// omits the key entirely (so pre-v2 readers and run outcomes stay
    /// byte-identical), and a line lacking the key deserializes to `None`.
    #[test]
    fn verifier_model_round_trips_and_is_omitted_when_absent() {
        let paired = RouteOutcomeRecord::new("main", "turn", "claude-opus-4-8", "completed")
            .with_signal("verdict")
            .with_signal_weight(Some(1.0))
            .with_verifier_model(Some("gpt-5.6-sol".to_string()));
        let json = serde_json::to_string(&paired).expect("serialize");
        assert!(json.contains("\"verifierModel\":\"gpt-5.6-sol\""), "{json}");
        let round_tripped: RouteOutcomeRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round_tripped, paired);
        assert_eq!(round_tripped.verifier_model.as_deref(), Some("gpt-5.6-sol"));

        // No verifier: the key must not appear at all (Option::is_none skip).
        let unpaired = RouteOutcomeRecord::new("subagent", "Explore", "gpt-5.6-sol", "completed");
        let json = serde_json::to_string(&unpaired).expect("serialize");
        assert!(!json.contains("verifierModel"), "absent verifier must omit the key: {json}");

        // A pre-v2/no-verifier line deserializes with `None`, and blank input
        // is filtered to `None` by the builder.
        let parsed: RouteOutcomeRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.verifier_model, None);
        let blanked = RouteOutcomeRecord::new("main", "turn", "m", "completed")
            .with_verifier_model(Some("   ".to_string()));
        assert_eq!(blanked.verifier_model, None);
    }

    /// Backward-compat: 10 REAL lines captured from a live
    /// `route-outcomes.jsonl` (pre-v2 schema — no `role`/`complexity`/`risk`/
    /// `effortLevel`/`durationMs`/`routeSource`/`signalWeight` fields at all).
    /// Every v2 field must deserialize to `None` and every line must parse —
    /// the store must never need a migration to accept the schema change.
    /// This is a fixed string fixture (NOT a read of the live file at test
    /// time), so it stays reproducible independent of the developer's
    /// machine/session state.
    const LIVE_PRE_V2_LINES: &str = r#"{"recordedAt":1783010203,"routeKey":"subagent:Explore","targetKind":"subagent","target":"Explore","selectedModel":"gemini-3-pro","requestedModel":"gemini-3-pro","status":"failed","providerErrorClass":"invalidToolSchema"}
{"recordedAt":1783011453,"routeKey":"subagent:Explore","targetKind":"subagent","target":"Explore","selectedModel":"gpt-5.5","requestedModel":"gpt-5.5","status":"completed","outputTokens":4306}
{"recordedAt":1783044777,"routeKey":"subagent:code-reviewer","targetKind":"subagent","target":"code-reviewer","selectedModel":"gpt-5.3-codex-spark","requestedModel":"gpt-5.3-codex-spark","status":"completed","outputTokens":570}
{"recordedAt":1783173713,"routeKey":"subagent:code-reviewer","targetKind":"subagent","target":"code-reviewer","selectedModel":"gpt-5.5-fast","requestedModel":"gpt-5.5-fast","status":"completed","outputTokens":4580}
{"recordedAt":1783305219,"routeKey":"subagent:code-reviewer","targetKind":"subagent","target":"code-reviewer","selectedModel":"gpt-5.5-fast","requestedModel":"gpt-5.5-fast","status":"completed","outputTokens":12177}
{"recordedAt":1783435871,"routeKey":"subagent:debugger","targetKind":"subagent","target":"debugger","selectedModel":"gpt-5.5-fast","requestedModel":"gpt-5.5-fast","status":"completed","outputTokens":2985}
{"recordedAt":1783500533,"routeKey":"subagent:Explore","targetKind":"subagent","target":"Explore","selectedModel":"gpt-5.5-fast","requestedModel":"gpt-5.5-fast","status":"completed","outputTokens":9431}
{"recordedAt":1783606855,"routeKey":"subagent:Plan","targetKind":"subagent","target":"Plan","selectedModel":"claude-opus-4-8","requestedModel":"gpt-5.5-fast","status":"failed","providerErrorClass":"nonRetryable"}
{"recordedAt":1783613059,"routeKey":"subagent:code-reviewer","targetKind":"subagent","target":"code-reviewer","selectedModel":"gpt-5.5-fast","requestedModel":"gpt-5.5-fast","status":"completed","outputTokens":13112}
{"recordedAt":1783621615,"routeKey":"subagent:code-reviewer","targetKind":"subagent","target":"code-reviewer","selectedModel":"gpt-5.5-fast","requestedModel":"gpt-5.5-fast","status":"completed","outputTokens":8291}"#;

    #[test]
    fn route_outcome_v2_schema_parses_pre_v2_live_records() {
        let records: Vec<RouteOutcomeRecord> = LIVE_PRE_V2_LINES
            .lines()
            .map(|line| serde_json::from_str(line).expect("pre-v2 live line must still parse"))
            .collect();

        assert_eq!(records.len(), 10);
        for record in &records {
            assert_eq!(record.role, None);
            assert_eq!(record.complexity, None);
            assert_eq!(record.risk, None);
            assert_eq!(record.effort_level, None);
            assert_eq!(record.duration_ms, None);
            assert_eq!(record.route_source, None);
            assert_eq!(record.signal_weight, None);
            assert_eq!(record.verifier_model, None);
            assert_eq!(record.decision, None);
            assert_eq!(record.decision_width, None);
            assert_eq!(record.reworked, None);
            assert_eq!(record.intervened, None);
            assert_eq!(record.verdict_basis, None);
            assert_eq!(record.verdict_subject, None);
            assert_eq!(record.run_id, None, "a legacy line names no attempt");
            assert_eq!(
                record.parent_attempt, None,
                "a legacy line names no attempt it served"
            );
            assert_eq!(record.shape, None, "a legacy line names no plan shape");
            assert_eq!(
                record.shape_kind(),
                None,
                "an absent label is not a shape — never `solo` by default"
            );
        }
        // Sanity: the fixture still carries the real field values through the
        // v2 struct shape (the whole point of `#[serde(default)]`, not just
        // "does it fail to error out").
        assert_eq!(records[0].selected_model, "gemini-3-pro");
        assert_eq!(records[0].provider_error_class.as_deref(), Some("invalidToolSchema"));
        assert_eq!(records[7].requested_model.as_deref(), Some("gpt-5.5-fast"));
        assert_eq!(records[7].selected_model, "claude-opus-4-8");

        // And the whole fixture summarizes without error through the SAME
        // (unchanged) public entry point the store's readers use.
        let summary = summarize_route_outcomes(&records);
        assert_eq!(summary.total, 10);

        // The other direction: a record written TODAY round-trips its join
        // columns, and an empty one still costs no bytes on a file kept
        // forever.
        let written = RouteOutcomeRecord::new("subagent", "x", "m", OUTCOME_COMPLETED)
            .with_attempt_key("agent-1#2")
            .with_parent_attempt("session-3@8")
            .with_shape(PlanShape::Parallel { width: 4 });
        let line = serde_json::to_string(&written).expect("serialize");
        let read: RouteOutcomeRecord =
            serde_json::from_str(&line).expect("a fresh record round-trips");
        assert_eq!(read.run_id.as_deref(), Some("agent-1#2"));
        assert_eq!(read.parent_attempt.as_deref(), Some("session-3@8"));
        assert_eq!(read.shape_kind(), Some(PlanShape::Parallel { width: 4 }));
        let bare = serde_json::to_string(&RouteOutcomeRecord::new(
            "subagent",
            "x",
            "m",
            OUTCOME_COMPLETED,
        ))
        .expect("serialize");
        for absent in ["parentAttempt", "shape"] {
            assert!(!bare.contains(absent), "{absent} must not be written empty: {bare}");
        }
    }

    // --- P1 decision-surface aggregation ------------------------------------

    #[test]
    fn decision_kind_registry_is_the_single_source_of_kinds() {
        // no-hardcoding contract: the kinds live in ONE table; round-trip each
        // label so a writer and the aggregator can never drift apart.
        assert_eq!(DecisionKind::ALL.len(), 6);
        for kind in DecisionKind::ALL {
            assert_eq!(DecisionKind::from_label(kind.as_str()), Some(kind));
        }
        assert_eq!(DecisionKind::from_label("model"), Some(DecisionKind::Model));
        assert_eq!(DecisionKind::from_label("classify"), Some(DecisionKind::Classify));
        assert_eq!(DecisionKind::from_label("no-such-kind"), None);
        // Bookkeeping kinds are exactly the two that are not attempts at work.
        let bookkeeping: Vec<_> = DecisionKind::ALL
            .into_iter()
            .filter(|kind| kind.is_bookkeeping())
            .collect();
        assert_eq!(bookkeeping, vec![DecisionKind::Fold, DecisionKind::Classify]);
    }

    #[test]
    fn route_tax_calls_round_trip_through_one_table() {
        assert_eq!(RouteTaxCall::ALL.len(), 2);
        for call in RouteTaxCall::ALL {
            assert_eq!(RouteTaxCall::from_label(call.as_str()), Some(call));
        }
        assert_eq!(RouteTaxCall::from_label("no-such-call"), None);
    }

    /// The tax row is filed away from every work route: one route key of its
    /// own, the Classify decision, and the call as its target. A writer that
    /// had to spell any of those itself would be the second place they live.
    #[test]
    fn a_route_tax_row_is_filed_under_its_own_key_and_decision() {
        let record = RouteOutcomeRecord::route_tax(
            RouteTaxCall::Probe,
            "fast-model",
            OUTCOME_COMPLETED,
        )
        .with_attempt_key("session-9@4")
        .with_duration_ms(Some(870))
        .with_output_tokens(140);

        assert_eq!(record.route_key, ROUTE_TAX_ROUTE_KEY);
        assert_eq!(record.target, RouteTaxCall::Probe.as_str());
        assert_eq!(record.decision_kind(), DecisionKind::Classify);
        assert!(record.decision_kind().is_bookkeeping());
        assert_eq!(record.run_id.as_deref(), Some("session-9@4"));
        assert_eq!(record.duration_ms, Some(870));
        assert_eq!(record.output_tokens, 140);
        // The tax says nothing about a work route, so it carries no route
        // provenance to be mistaken for one.
        assert_eq!(record.role, None);
        assert_eq!(record.complexity, None);
        assert_eq!(record.route_source, None);
    }

    /// The timed-out probe is `stopped`, not `failed`: a wall cut it off, and
    /// nothing was learned about the answer it would have given.
    #[test]
    fn the_three_terminal_statuses_are_named_once() {
        for status in [OUTCOME_COMPLETED, OUTCOME_FAILED, OUTCOME_STOPPED] {
            assert!(is_terminal_outcome_status(status), "{status}");
        }
        assert!(!is_terminal_outcome_status("still_running"));
    }

    /// The spawn grammar is minted in exactly one function; `with_attempt` is
    /// a caller of it, not a second copy.
    #[test]
    fn the_spawn_attempt_grammar_has_one_speller() {
        let record = RouteOutcomeRecord::new("subagent", "x", "m", OUTCOME_COMPLETED)
            .with_attempt("agent-7", 3);
        assert_eq!(
            record.run_id,
            super::super::spawn_attempt_key("agent-7", 3),
            "the builder must not spell the grammar a second time"
        );
        let blank = RouteOutcomeRecord::new("subagent", "x", "m", OUTCOME_COMPLETED)
            .with_attempt("  ", 3);
        assert_eq!(blank.run_id, None);
    }

    #[test]
    fn plan_shape_labels_round_trip_through_one_grammar() {
        let shapes = [
            PlanShape::Solo,
            PlanShape::HostPrelude { width: 4 },
            PlanShape::Delegate,
            PlanShape::Parallel { width: 2 },
        ];
        for shape in shapes {
            assert_eq!(PlanShape::from_label(&shape.label()), Some(shape), "{shape:?}");
        }
        assert_eq!(PlanShape::Solo.label(), "solo");
        assert_eq!(PlanShape::HostPrelude { width: 4 }.label(), "host-prelude:4");
        assert_eq!(PlanShape::Parallel { width: 2 }.lanes(), 2);
        assert_eq!(PlanShape::Delegate.lanes(), 1);
        // A lane shape needs a width of at least one; a solo shape takes none.
        assert_eq!(PlanShape::from_label("parallel"), None);
        assert_eq!(PlanShape::from_label("parallel:0"), None);
        assert_eq!(PlanShape::from_label("parallel:x"), None);
        assert_eq!(PlanShape::from_label("solo:2"), None);
        assert_eq!(PlanShape::from_label("no-such-shape"), None);
    }

    #[test]
    fn attempt_and_parent_keys_are_stored_verbatim_and_never_empty() {
        let record = RouteOutcomeRecord::new("main", "turn", "m", "completed")
            .with_attempt_key("  session-1@7 ")
            .with_parent_attempt("agent-9#2")
            .with_shape(PlanShape::HostPrelude { width: 3 });
        assert_eq!(record.run_id.as_deref(), Some("session-1@7"));
        assert_eq!(record.parent_attempt.as_deref(), Some("agent-9#2"));
        assert_eq!(record.shape_kind(), Some(PlanShape::HostPrelude { width: 3 }));

        let blank = RouteOutcomeRecord::new("main", "turn", "m", "completed")
            .with_attempt_key("   ")
            .with_parent_attempt("")
            .with_shape_label("no-such-shape");
        assert_eq!(blank.run_id, None);
        assert_eq!(blank.parent_attempt, None);
        assert_eq!(blank.shape_kind(), None, "an unknown label stamps nothing");
        let by_label = RouteOutcomeRecord::new("main", "turn", "m", "completed").with_shape_label("parallel:3");
        assert_eq!(by_label.shape_kind(), Some(PlanShape::Parallel { width: 3 }));

        // The spawn grammar and the spelled-out key meet on equality.
        let spawn = RouteOutcomeRecord::new("subagent", "Explore", "m", "completed")
            .with_attempt("agent-9", 2);
        assert_eq!(spawn.run_id, record.parent_attempt);

        // Absent fields serialize to nothing, so old readers see old lines.
        let line = serde_json::to_string(&blank).expect("serialize");
        assert!(!line.contains("parentAttempt") && !line.contains("parent_attempt"));
        assert!(!line.contains("shape"));
    }

    #[test]
    fn classify_rows_teach_nothing_and_speak_for_no_attempt() {
        // A probe paid by attempt `session-1@3` and the turn's own verdict:
        // the tax row must not become the attempt's speaker nor a sample.
        let tax = RouteOutcomeRecord::new("main", "probe", "fast-model", "completed")
            .with_decision(DecisionKind::Classify)
            .with_attempt_key("session-1@3");
        let verdict = RouteOutcomeRecord::new("main", "turn", "big-model", "failed")
            .with_signal(VERDICT_SIGNAL)
            .with_decision(DecisionKind::Verify)
            .with_attempt_key("session-1@3");
        let records = vec![tax, verdict];
        let mask = learning_sample_mask(&records);
        assert_eq!(mask, vec![false, true]);
        let summary = summarize_route_outcomes(&records);
        assert_eq!(summary.total, 1);
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn decisions_split_by_kind_and_surface_the_weakest() {
        let mut records = Vec::new();
        // model: 8 completed (rate 1.0)
        for _ in 0..8 {
            records.push(
                RouteOutcomeRecord::new("subagent", "Plan", "m", "completed")
                    .with_decision(DecisionKind::Model),
            );
        }
        // verify: 2 completed, 6 model-fault failed (rate 0.25) — the weakest
        for _ in 0..2 {
            records.push(
                RouteOutcomeRecord::new("main", "turn", "m", "completed")
                    .with_decision(DecisionKind::Verify),
            );
        }
        for _ in 0..6 {
            records.push(
                RouteOutcomeRecord::new("main", "turn", "m", "failed")
                    .with_decision(DecisionKind::Verify),
            );
        }
        // decompose: 4 completed, 1 failed (rate 0.8), width carried
        for _ in 0..4 {
            records.push(
                RouteOutcomeRecord::new("host", "fanout", "m", "completed")
                    .with_decision(DecisionKind::Decompose)
                    .with_decision_width(Some(4)),
            );
        }
        records.push(
            RouteOutcomeRecord::new("host", "fanout", "m", "failed")
                .with_decision(DecisionKind::Decompose)
                .with_decision_width(Some(4)),
        );
        // agent: 3 completed (rate 1.0)
        for _ in 0..3 {
            records.push(
                RouteOutcomeRecord::new("subagent", "code-reviewer", "m", "completed")
                    .with_decision(DecisionKind::Agent),
            );
        }
        // fold: 2 completed (rate 1.0), one marked reworked
        records.push(
            RouteOutcomeRecord::new("host", "fold", "m", "completed")
                .with_decision(DecisionKind::Fold)
                .with_reworked(true),
        );
        records.push(
            RouteOutcomeRecord::new("host", "fold", "m", "completed")
                .with_decision(DecisionKind::Fold),
        );
        // a cancelled verify (nondecisive) must not move the verify rate
        records.push(
            RouteOutcomeRecord::new("main", "turn", "m", "stopped")
                .with_decision(DecisionKind::Verify),
        );

        let stats = summarize_decisions_by_kind(&records);
        // all five kinds present, in canonical order
        let labels: Vec<&str> = stats.iter().map(|stat| stat.decision.as_str()).collect();
        assert_eq!(labels, vec!["model", "agent", "decompose", "verify", "fold"]);

        let verify = stats.iter().find(|stat| stat.decision == "verify").unwrap();
        assert_eq!(verify.completed, 2);
        assert_eq!(verify.failed, 6);
        assert_eq!(verify.nondecisive, 1, "the stopped verify is excluded from decisive");
        assert!((verify.success_rate().unwrap() - 0.25).abs() < 1e-9);

        let fold = stats.iter().find(|stat| stat.decision == "fold").unwrap();
        assert_eq!(fold.reworked, 1);

        let weakest = weakest_decision_kind(&stats, 2).unwrap();
        assert_eq!(
            weakest.decision, "verify",
            "verify has the lowest decisive success rate"
        );
    }

    #[test]
    fn absent_decision_counts_as_model_the_historical_default() {
        // A plain record (no decision axis — every record written before P1)
        // tallies under `model`, so the aggregation never loses old evidence.
        let records = vec![
            RouteOutcomeRecord::new("subagent", "Plan", "m", "completed"),
            RouteOutcomeRecord::new("subagent", "Plan", "m", "failed"),
        ];
        let stats = summarize_decisions_by_kind(&records);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].decision, "model");
        assert_eq!(stats[0].total, 2);
        assert_eq!(records[0].decision_kind(), DecisionKind::Model);
    }

    #[test]
    fn weakest_decision_kind_ignores_kinds_below_the_sample_floor() {
        // agent: 1 decisive, failed (rate 0.0) but below the floor.
        // model: 4 decisive, rate 0.5. With floor 2, model wins (agent is noise).
        let stats = summarize_decisions_by_kind(&[
            RouteOutcomeRecord::new("s", "a", "m", "failed").with_decision(DecisionKind::Agent),
            RouteOutcomeRecord::new("s", "b", "m", "completed").with_decision(DecisionKind::Model),
            RouteOutcomeRecord::new("s", "b", "m", "completed").with_decision(DecisionKind::Model),
            RouteOutcomeRecord::new("s", "b", "m", "failed").with_decision(DecisionKind::Model),
            RouteOutcomeRecord::new("s", "b", "m", "failed").with_decision(DecisionKind::Model),
        ]);
        assert_eq!(weakest_decision_kind(&stats, 2).unwrap().decision, "model");
        // With floor 1, the single 0.0 agent sample now qualifies and wins.
        assert_eq!(weakest_decision_kind(&stats, 1).unwrap().decision, "agent");
        // No stats at all -> None.
        assert!(weakest_decision_kind(&[], 2).is_none());
    }

    #[test]
    fn decision_fields_round_trip_and_omit_when_absent() {
        let stamped = RouteOutcomeRecord::new("host", "fanout", "m", "completed")
            .with_decision(DecisionKind::Decompose)
            .with_decision_width(Some(4))
            .with_reworked(true)
            .with_intervened(true);
        let json = serde_json::to_string(&stamped).expect("serialize");
        assert!(json.contains("\"decision\":\"decompose\""), "{json}");
        assert!(json.contains("\"decisionWidth\":4"), "{json}");
        assert!(json.contains("\"reworked\":true"), "{json}");
        assert!(json.contains("\"intervened\":true"), "{json}");
        let round_tripped: RouteOutcomeRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round_tripped, stamped);
        assert_eq!(round_tripped.decision_kind(), DecisionKind::Decompose);

        // A record with no decision axis omits every new key (pre-P1 readers
        // and run outcomes stay byte-identical).
        let plain = RouteOutcomeRecord::new("subagent", "Plan", "m", "completed");
        let json = serde_json::to_string(&plain).expect("serialize");
        for key in ["decision", "decisionWidth", "reworked", "intervened"] {
            assert!(!json.contains(key), "absent {key} must be omitted: {json}");
        }
    }

    // --- P2 verify metrics + verdict basis ----------------------------------

    #[test]
    fn verdict_basis_round_trips_and_defaults_to_model_when_absent() {
        assert_eq!(VerdictBasis::ALL.len(), 2);
        for basis in VerdictBasis::ALL {
            assert_eq!(VerdictBasis::from_label(basis.as_str()), Some(basis));
        }
        assert_eq!(resolve_verdict_basis(true), VerdictBasis::Objective);
        assert_eq!(resolve_verdict_basis(false), VerdictBasis::Model);

        let stamped = RouteOutcomeRecord::new("main", "turn", "m", "completed")
            .with_decision(DecisionKind::Verify)
            .with_verdict_basis(VerdictBasis::Objective);
        let json = serde_json::to_string(&stamped).expect("serialize");
        assert!(json.contains("\"verdictBasis\":\"objective\""), "{json}");
        let round_tripped: RouteOutcomeRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round_tripped, stamped);
        assert_eq!(round_tripped.verdict_basis_kind(), VerdictBasis::Objective);

        // A bare verify record (no basis stamped) reads as model, and omits the
        // key on the wire.
        let bare = RouteOutcomeRecord::new("main", "turn", "m", "completed")
            .with_decision(DecisionKind::Verify);
        assert_eq!(bare.verdict_basis_kind(), VerdictBasis::Model);
        assert!(!serde_json::to_string(&bare).unwrap().contains("verdictBasis"));
    }

    #[test]
    fn verify_metrics_count_catches_and_basis_over_verify_records_only() {
        let records = vec![
            // 4 verify decisions: 1 caught a defect (failed), 3 passed.
            RouteOutcomeRecord::new("main", "turn", "m", "failed")
                .with_decision(DecisionKind::Verify)
                .with_verdict_basis(VerdictBasis::Model),
            RouteOutcomeRecord::new("main", "turn", "m", "completed")
                .with_decision(DecisionKind::Verify)
                .with_verdict_basis(VerdictBasis::Model),
            RouteOutcomeRecord::new("main", "turn", "m", "completed")
                .with_decision(DecisionKind::Verify)
                .with_verdict_basis(VerdictBasis::Objective),
            // A verify record with no basis reads as model_only.
            RouteOutcomeRecord::new("main", "turn", "m", "completed")
                .with_decision(DecisionKind::Verify),
            // An infra fault on a verify run is NOT a catch (excluded).
            RouteOutcomeRecord::new("main", "turn", "m", "failed")
                .with_decision(DecisionKind::Verify)
                .with_provider_error_class(Some("rateLimit".to_string())),
            // A non-verify (model) decision must be ignored entirely.
            RouteOutcomeRecord::new("subagent", "Plan", "m", "failed")
                .with_decision(DecisionKind::Model),
        ];

        let metrics = verify_metrics(&records);
        assert_eq!(metrics.total, 5, "only the five verify records count");
        assert_eq!(metrics.caught, 1, "the infra-fault verify is not a catch");
        assert_eq!(metrics.objective, 1);
        assert_eq!(metrics.model_only, 4, "3 model + 1 absent + 1 infra all read as model");
        assert!((metrics.catch_rate().unwrap() - 0.2).abs() < 1e-9);
        assert!((metrics.model_only_rate().unwrap() - 0.8).abs() < 1e-9);

        // No verify records at all -> rates are None, not a divide-by-zero.
        let empty = verify_metrics(&[RouteOutcomeRecord::new("s", "a", "m", "completed")]);
        assert_eq!(empty.total, 0);
        assert_eq!(empty.catch_rate(), None);
        assert_eq!(empty.model_only_rate(), None);
    }

    /// A validator whose own output was unusable records a failed verify
    /// against ITSELF; that is not a defect the work had, so it must not count
    /// as a catch — it is a validator fault (re-verification finding).
    #[test]
    fn a_validator_fault_is_not_a_caught_defect() {
        assert_eq!(VerdictSubject::ALL.len(), 2);
        for subject in VerdictSubject::ALL {
            assert_eq!(VerdictSubject::from_label(subject.as_str()), Some(subject));
        }
        let records = vec![
            // a real catch: the work failed review
            RouteOutcomeRecord::new("subagent", "Refactor", "m", "failed")
                .with_decision(DecisionKind::Verify),
            // the validator garbled its own output
            RouteOutcomeRecord::new("subagent", "code-reviewer", "m", "failed")
                .with_decision(DecisionKind::Verify)
                .with_verdict_subject(VerdictSubject::Validator),
            RouteOutcomeRecord::new("subagent", "Refactor", "m", "completed")
                .with_decision(DecisionKind::Verify),
        ];
        let metrics = verify_metrics(&records);
        assert_eq!(metrics.total, 3);
        assert_eq!(metrics.caught, 1, "only the work's failure is a catch");
        assert_eq!(metrics.validator_faults, 1);
        assert_eq!(records[0].verdict_subject_kind(), VerdictSubject::Work, "absent == work");
        let json = serde_json::to_string(&records[1]).unwrap();
        assert!(json.contains("\"verdictSubject\":\"validator\""), "{json}");
        assert!(!serde_json::to_string(&records[0]).unwrap().contains("verdictSubject"));
    }

    // --- re-verification findings against the LIVE stores (2026-09-10) ------

    /// Every verdict written before the P1 decision axis existed carries
    /// `signal:"verdict"` and NO `decision` field — 457 real records across
    /// the live stores. They ARE verify decisions in the pre-P1 vocabulary, so
    /// the reader must infer Verify from the legacy signal instead of folding
    /// them into `model` and throwing the whole historical verify sample away.
    #[test]
    fn legacy_verdict_signal_without_decision_reads_as_verify() {
        let legacy_fail = RouteOutcomeRecord::new("subagent", "code-reviewer", "m", "failed")
            .with_signal("verdict");
        let legacy_pass = RouteOutcomeRecord::new("subagent", "code-reviewer", "m", "completed")
            .with_signal("verdict");
        let plain_run = RouteOutcomeRecord::new("subagent", "code-reviewer", "m", "completed");

        assert_eq!(legacy_fail.decision_kind(), DecisionKind::Verify);
        assert_eq!(plain_run.decision_kind(), DecisionKind::Model, "a run outcome stays model");

        let stats = summarize_decisions_by_kind(&[legacy_fail.clone(), legacy_pass.clone(), plain_run]);
        let labels: Vec<&str> = stats.iter().map(|stat| stat.decision.as_str()).collect();
        assert_eq!(labels, vec!["model", "verify"], "legacy verdicts land in the verify bucket");

        let metrics = verify_metrics(&[legacy_fail, legacy_pass]);
        assert_eq!(metrics.total, 2, "both legacy verdicts count as verify decisions");
        assert_eq!(metrics.caught, 1);
    }

    /// Ranking "where is orchestration going wrong" by RAW rate lets two thin
    /// failures (0-of-2, rate 0.0) outrank a well-evidenced disaster (10-of-100,
    /// rate 0.1). The module already owns the answer — the confidence ramp
    /// `RouteOutcomeBucket::feedback_adjustment` uses — so the weakest kind must
    /// be the one with the worst confidence-weighted margin, not the worst raw
    /// rate at the floor (re-verification finding, 2026-09-10).
    #[test]
    fn weakest_kind_ranks_by_confidence_weighted_margin_not_raw_rate() {
        let mut records = Vec::new();
        // verify: 0 completed, 2 failed -> raw 0.0 but only two samples
        for _ in 0..2 {
            records.push(
                RouteOutcomeRecord::new("main", "turn", "m", "failed").with_decision(DecisionKind::Verify),
            );
        }
        // model: 10 completed, 90 failed -> raw 0.1 on a hundred samples
        for _ in 0..10 {
            records.push(
                RouteOutcomeRecord::new("subagent", "Plan", "m", "completed").with_decision(DecisionKind::Model),
            );
        }
        for _ in 0..90 {
            records.push(
                RouteOutcomeRecord::new("subagent", "Plan", "m", "failed").with_decision(DecisionKind::Model),
            );
        }
        let stats = summarize_decisions_by_kind(&records);
        let weakest = weakest_decision_kind(&stats, 2).unwrap();
        assert_eq!(
            weakest.decision, "model",
            "ninety evidenced failures are the real problem, not two thin ones"
        );
    }

    // --- one attempt, one learning sample (t-3920) -------------------------

    fn run(target: &str, model: &str, status: &str, attempt: &str) -> RouteOutcomeRecord {
        RouteOutcomeRecord::new("subagent", target, model, status)
            .with_decision(DecisionKind::Model)
            .with_attempt(attempt, 1)
    }

    fn verdict(target: &str, model: &str, passed: bool, attempt: &str) -> RouteOutcomeRecord {
        let status = if passed { "completed" } else { "failed" };
        RouteOutcomeRecord::new("subagent", target, model, status)
            .with_signal(VERDICT_SIGNAL)
            .with_decision(DecisionKind::Verify)
            .with_attempt(attempt, 1)
    }

    /// Four attempts that all finished and all failed verification. Per
    /// receipt the route read 4-4 — a zero margin, so a model whose work is
    /// always wrong kept its standing on completions alone. Per attempt the
    /// verdict speaks: 0-4, a real demotion, in every feedback path.
    #[test]
    fn a_completion_whose_work_failed_verification_never_nets_success() {
        let mut records = Vec::new();
        for index in 0..4 {
            let attempt = format!("agent-{index}");
            records.push(run("Refactor", "model-a", "completed", &attempt));
            records.push(verdict("Refactor", "model-a", false, &attempt));
        }

        let summary = summarize_route_outcomes(&records);
        let bucket = &summary.by_route[0];
        assert_eq!((bucket.completed, bucket.failed), (0, 4));
        assert_eq!(
            summary.decisive_counts_for_route_key("subagent:Refactor"),
            vec![("model-a".to_string(), 4)],
            "one decisive sample per attempt, not per receipt"
        );
        // (0 - 4) * 120 * 4 / (4 * 8): half the bound, where per-receipt
        // counting gave exactly 0.
        let hint = summary.feedback_hint_for_route_key("subagent:Refactor");
        assert_eq!(hint.bounded_adjustment_for("model-a"), -(MAX_FEEDBACK_ADJUSTMENT / 2));
        let weighted = weighted_feedback_hint_for_route_key(
            &records,
            "subagent:Refactor",
            epoch_seconds_now(),
            ToString::to_string,
        );
        assert!(weighted.bounded_adjustment_for("model-a") < 0);
    }

    /// A validator whose output was unusable is recorded against its OWN
    /// attempt. That attempt finished, but it did not do its job: the fault
    /// speaks for it, so it can never net the win its completion claimed.
    /// The accuracy report still reads it as a validator fault, not a catch.
    #[test]
    fn an_unusable_verification_cannot_earn_success() {
        let mut records = Vec::new();
        for attempt in ["validator-1", "validator-2"] {
            records.push(run("code-reviewer", "validator-model", "completed", attempt));
            records.push(
                verdict("code-reviewer", "validator-model", false, attempt)
                    .with_verdict_subject(VerdictSubject::Validator),
            );
        }

        let summary = summarize_route_outcomes(&records);
        assert_eq!((summary.by_route[0].completed, summary.by_route[0].failed), (0, 2));
        assert!(summary.feedback_hint_for_route_key("subagent:code-reviewer").bounded_adjustment_for("validator-model") < 0);
        let metrics = verify_metrics(&records);
        assert_eq!((metrics.caught, metrics.validator_faults), (0, 2));
    }

    fn unverified(target: &str, model: &str, attempt: &str) -> RouteOutcomeRecord {
        RouteOutcomeRecord::new("subagent", target, model, "stopped")
            .with_signal(VERDICT_SIGNAL)
            .with_decision(DecisionKind::Verify)
            .with_attempt(attempt, 1)
    }

    /// A verifier that crashed, or answered nothing usable, did not judge the
    /// work. The implementer's attempt is UNVERIFIED: its bare completion no
    /// longer counts as a win and nothing counts against it — only an actual
    /// failed verdict about the same attempt does, before or after.
    #[test]
    fn an_unavailable_verification_withdraws_credit_without_blame() {
        let records = vec![
            run("Refactor", "model-a", "completed", "agent-1"),
            unverified("Refactor", "model-a", "agent-1"),
            run("Refactor", "model-a", "completed", "agent-2"),
            unverified("Refactor", "model-a", "agent-2"),
            verdict("Refactor", "model-a", false, "agent-2"),
            run("Refactor", "model-a", "completed", "agent-3"),
            verdict("Refactor", "model-a", false, "agent-3"),
            unverified("Refactor", "model-a", "agent-3"),
        ];

        assert_eq!(
            learning_sample_mask(&records),
            vec![false, true, false, false, true, false, true, false]
        );
        let summary = summarize_route_outcomes(&records);
        let bucket = &summary.by_route[0];
        assert_eq!(
            (bucket.completed, bucket.failed, bucket.stopped),
            (0, 2, 1),
            "agent-1 is unverified (no win, no loss); agent-2 and agent-3 failed review"
        );
    }

    /// A restated verdict about one attempt is the same evidence, not new
    /// evidence; and once a check has caught a failure, a later pass on the
    /// SAME attempt does not clear it.
    #[test]
    fn verdicts_about_one_attempt_count_once() {
        let records = vec![
            run("Refactor", "model-a", "completed", "agent-1"),
            verdict("Refactor", "model-a", true, "agent-1"),
            verdict("Refactor", "model-a", true, "agent-1"),
            run("Refactor", "model-a", "completed", "agent-2"),
            verdict("Refactor", "model-a", false, "agent-2"),
            verdict("Refactor", "model-a", true, "agent-2"),
        ];

        assert_eq!(
            learning_sample_mask(&records),
            vec![false, false, true, false, true, false]
        );
        let summary = summarize_route_outcomes(&records);
        let bucket = &summary.by_route[0];
        assert_eq!((bucket.total, bucket.completed, bucket.failed), (2, 1, 1));
    }

    /// Two runs of one agent (a resume advances the generation) and a second
    /// agent are three attempts — three samples, none merged.
    #[test]
    fn distinct_attempts_stay_distinct() {
        let first = RouteOutcomeRecord::new("subagent", "Refactor", "model-a", "completed")
            .with_decision(DecisionKind::Model)
            .with_attempt("agent-1", 1);
        let resumed = first.clone().with_attempt("agent-1", 2);
        let other = first.clone().with_attempt("agent-2", 1);
        assert_ne!(first.run_id, resumed.run_id);
        let records = vec![first, resumed, other];

        assert_eq!(learning_sample_mask(&records), vec![true, true, true]);
        assert_eq!(summarize_route_outcomes(&records).by_route[0].completed, 3);
    }

    /// Records with no attempt (every line written before `runId`, and the
    /// main-turn verdicts that have no spawn attempt) are never guessed into
    /// one: each still counts on its own, exactly as before.
    #[test]
    fn unattributed_records_are_never_merged() {
        let legacy: Vec<RouteOutcomeRecord> = LIVE_PRE_V2_LINES
            .lines()
            .map(|line| serde_json::from_str(line).expect("live line"))
            .collect();
        assert!(learning_sample_mask(&legacy).into_iter().all(|learns| learns));

        let unattributed = vec![
            RouteOutcomeRecord::new("subagent", "Refactor", "model-a", "completed"),
            RouteOutcomeRecord::new("subagent", "Refactor", "model-a", "failed").with_signal(VERDICT_SIGNAL),
            RouteOutcomeRecord::new("main", "turn", "model-a", "completed")
                .with_signal(VERDICT_SIGNAL)
                .with_decision(DecisionKind::Verify),
        ];
        assert_eq!(learning_sample_mask(&unattributed), vec![true, true, true]);
        let bucket = summarize_route_outcomes(&unattributed)
            .by_route
            .into_iter()
            .find(|bucket| bucket.route_key == "subagent:Refactor")
            .expect("refactor bucket");
        assert_eq!((bucket.completed, bucket.failed), (1, 1));
    }

    /// A fold is the accuracy report's wasted-spawn accounting. The lane's
    /// run already has its receipt; the fold must not hand it a second win
    /// (with or without an attempt), while the report still counts it.
    #[test]
    fn a_fold_never_teaches_the_router() {
        let records = vec![
            run("Explore", "lane-model", "completed", "lane-1"),
            RouteOutcomeRecord::new("subagent", "Explore", "lane-model", "completed")
                .with_signal("fold")
                .with_decision(DecisionKind::Fold)
                .with_attempt("lane-1", 1),
            RouteOutcomeRecord::new("subagent", "Explore", "lane-model", "completed")
                .with_decision(DecisionKind::Fold),
        ];

        assert_eq!(learning_sample_mask(&records), vec![true, false, false]);
        let summary = summarize_route_outcomes(&records);
        assert_eq!((summary.total, summary.by_route[0].completed), (1, 1));
        let folds = summarize_decisions_by_kind(&records)
            .into_iter()
            .find(|stat| stat.decision == DecisionKind::Fold.as_str())
            .expect("fold stat");
        assert_eq!(folds.total, 2, "the accuracy report still sees every fold");
    }

    #[test]
    fn with_attempt_names_the_durable_attempt_and_round_trips() {
        let record = RouteOutcomeRecord::new("subagent", "Refactor", "m", "completed")
            .with_attempt(" agent-1789236663739000000 ", 3);
        assert_eq!(record.run_id.as_deref(), Some("agent-1789236663739000000#3"));
        let json = serde_json::to_string(&record).expect("serialize");
        assert!(json.contains("\"runId\":\"agent-1789236663739000000#3\""), "{json}");
        let round_tripped: RouteOutcomeRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round_tripped, record);

        let blank = RouteOutcomeRecord::new("subagent", "Refactor", "m", "completed").with_attempt("  ", 1);
        assert_eq!(blank.run_id, None, "a blank agent id names no attempt");
        assert!(!serde_json::to_string(&blank).expect("serialize").contains("runId"));
    }

    /// Two recorders appending at once must never zipper their bytes together.
    /// The live stores held 5 such lines (each collision destroying BOTH
    /// records), because streaming `serde_json` straight into the File issued
    /// many small write()s. Every line landed by concurrent recorders must
    /// parse, and none may be lost. Buckets are kept distinct and under the
    /// retention caps so no prune rewrite runs — this isolates append-vs-append.
    #[test]
    fn concurrent_recorders_never_interleave_a_line() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("route-outcomes.jsonl");
        let threads = 8usize;
        let per_thread = 40usize; // < OUTCOME_BUCKET_RETENTION, so no prune rewrite
        let handles: Vec<_> = (0..threads)
            .map(|lane| {
                let path = path.clone();
                std::thread::spawn(move || {
                    // A long target = many serialized bytes, so an interleaving
                    // writer has plenty of seams to corrupt.
                    let target = format!("lane-{lane}-{}", "x".repeat(150));
                    for _ in 0..per_thread {
                        let record = RouteOutcomeRecord::new("subagent", &target, "m", "completed");
                        record_route_outcome_at_path(&path, &record).expect("record");
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("recorder thread");
        }

        // Read RAW lines (not the lenient reader, which silently drops a bad
        // line) so corruption is counted, not hidden.
        let raw = std::fs::read_to_string(&path).expect("read store");
        let lines: Vec<&str> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
        assert_eq!(lines.len(), threads * per_thread, "no line may be lost");
        let unparseable = lines
            .iter()
            .filter(|line| serde_json::from_str::<RouteOutcomeRecord>(line).is_err())
            .count();
        assert_eq!(unparseable, 0, "no line may be zippered by a concurrent recorder");
    }
    #[test]
    fn a_prebound_verdict_learns_the_same_attempts_final_model_after_a_swap() {
        let run = RouteOutcomeRecord::new("subagent", "Refactor", "actual-model", "completed")
            .with_attempt("agent-swap", 0);
        let verdict = RouteOutcomeRecord::new("subagent", "Refactor", "planned-model", "failed")
            .with_attempt("agent-swap", 0).with_decision(DecisionKind::Verify).with_signal("verdict");
        let resumed = RouteOutcomeRecord::new("subagent", "Refactor", "next-model", "completed")
            .with_attempt("agent-swap", 1);
        let rows = [run, verdict, resumed];
        let samples: Vec<_> = learning_samples(&rows).collect();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].selected_model, "actual-model");
        assert_eq!(samples[0].status, "failed");
        assert_eq!(samples[1].selected_model, "next-model");
        assert_eq!(rows[1].selected_model, "planned-model", "historical receipts are immutable");
    }

}
