use runtime::{RouteAutoClassifierMode, RouteConfidence, RouteShapeKind, RouteTaskComplexity};

use super::evidence::{infer_route_shape_evidence, shape_input_with_evidence, RouteEvidenceInput};
use super::settings::HostOrchestration;
use crate::fanout::{fanout_width_for, MIN_FANOUT_SUBTASKS};
use super::infer::{infer_route_role, mentions_design};
use super::metadata::{classify_task_metadata, task_complexity_verb_matched, TaskMetadataInput};
use super::planner::plan_agent_needs;
use super::probe_gate::ProbeGate;
use super::shape::select_route_shape;

/// Smart-routing assessment of a whole turn (the user's prompt). The host
/// orchestrator consults this to *drive* a route decision when its own
/// (multilingual) keyword classifier finds no delegation signal, so the smart
/// routing layer can influence orchestration rather than only model selection.
///
/// Pure and deterministic: it reads no settings and performs no IO, so it is
/// safe to call from the host's per-turn route classifier (which is unit-tested
/// as a pure function). Uses provider-free deterministic classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnOrchestrationHint {
    /// The canonical route shape the evidence suggests for this turn.
    pub shape: RouteShapeKind,
    /// How many distinct agent needs the planner found (0 → no delegation value).
    pub need_count: usize,
    /// Confidence in the suggested shape.
    pub confidence: RouteConfidence,
    /// Deterministic task-risk band from the same metadata classification.
    pub risk: runtime::RouteTaskRisk,
    /// The shape the person EXPLICITLY asked for in the turn text — the
    /// evidence pipeline's `requested_shape`, kept whole. Surfaced separately
    /// because the decision `shape` is the *natural* shape, which stays `Solo`
    /// whenever the need plan is empty — so an explicit request would
    /// otherwise be lost. Kept as the shape, not a yes/no: "one specialist"
    /// and "in parallel" are both delegation, but only one of them asks for
    /// lanes (t-3854).
    pub requested_shape: Option<RouteShapeKind>,
    /// The ONE model the person named in the turn's own words, as its
    /// canonical id ("fable 검토 받아" → `claude-fable-5-1`), or `None` when
    /// they named none — or more than one, which is a comparison, not a pin.
    /// The spawn door reads it as a person pin that outranks the assistant's
    /// `model` argument (2026-09-09: a Gemini parent read Fable as a persona
    /// and spawned the reviewer on the default Opus).
    pub user_named_model: Option<&'static str>,
}

impl TurnOrchestrationHint {
    /// The person asked to delegate in any shape ("use one specialist", "in
    /// parallel"). The same-model spawn guard honors it as "the user asked to
    /// delegate".
    #[must_use]
    pub fn user_requested_delegation(&self) -> bool {
        self.requested_shape
            .is_some_and(|requested| !matches!(requested, RouteShapeKind::Solo))
    }

    /// The person asked for NO delegation ("solo", "no agents"): the host must
    /// not pre-spawn on such a turn whatever the difficulty says.
    #[must_use]
    pub fn user_requested_solo(&self) -> bool {
        matches!(self.requested_shape, Some(RouteShapeKind::Solo))
    }
}

/// Whether a shape is independent lanes — the only shapes a parallel split
/// serves. One reading for both what the person asked for and what the
/// evidence found, so the two can never disagree about what "parallel" means.
fn shape_runs_lanes(shape: RouteShapeKind) -> bool {
    matches!(
        shape,
        RouteShapeKind::ParallelLanes | RouteShapeKind::ParallelRepairLoop
    )
}

/// The one model the person named in their prose, exactly.
///
/// Words are cut at anything that is not part of a model id (letters, digits,
/// `.`, `-`, `_`, `[`/`]` for `opus[1m]`), so a Korean particle glued to the
/// name (`opus로`) still yields the name. Each word is read by
/// [`api::exact_model_reference`] — exact aliases and ids only, no near-miss —
/// and only a single distinct model counts: "opus가 짠 코드를 fable로 검토해"
/// names two and pins neither.
/// The punctuation a model id may carry besides letters and digits —
/// `gpt-5.6`, `claude_x`, `opus[1m]`. Everything else, a Korean particle
/// included, ends the word.
const MODEL_ID_PUNCTUATION: [char; 5] = ['.', '-', '_', '[', ']'];
/// Punctuation that may trail a name in prose (`fable.`, `opus-`) and is
/// not part of it.
const MODEL_ID_TRAILING_PUNCTUATION: [char; 2] = ['.', '-'];

#[must_use]
pub fn user_named_model(user_text: &str) -> Option<&'static str> {
    let mut named: Option<&'static str> = None;
    for word in user_text
        .split(|c: char| !(c.is_ascii_alphanumeric() || MODEL_ID_PUNCTUATION.contains(&c)))
        .map(|word| word.trim_matches(MODEL_ID_TRAILING_PUNCTUATION))
        .filter(|word| !word.is_empty())
    {
        let Some(model) = api::exact_model_reference(word) else {
            continue;
        };
        match named {
            None => named = Some(model),
            Some(first) if first == model => {}
            Some(_) => return None,
        }
    }
    named
}

/// Classify a whole turn's task complexity with the same deterministic
/// classifier the per-agent router uses (role inference + metadata tables,
/// Korean parity, typo/coexisting-work guards — all pinned by the complexity
/// evaluation corpus). The host consults this for RESOURCE allocation only;
/// provenance-aware consumers may raise from this table verdict but never
/// lower effort from it. It never steers the model's own orchestration
/// judgment, which lives in the base prompt's delegation rubric.
#[must_use]
pub fn assess_turn_complexity(user_text: &str) -> runtime::RouteTaskComplexity {
    let role = infer_route_role(None, "", user_text);
    classify_task_metadata(&TaskMetadataInput::new(None, "", user_text), role).complexity
}

/// Provider-free assessment shared by headless and probe-fallback paths.
/// The deterministic tables classify complexity only; deliverable intent is a
/// model judgment owned exclusively by the probe, so the fallback is always
/// [`runtime::RouteTaskIntent::Other`].
#[must_use]
pub fn assess_turn_deterministic(user_text: &str) -> TurnProbeAssessment {
    let role = infer_route_role(None, "", user_text);
    let input = TaskMetadataInput::new(None, "", user_text);
    let metadata = classify_task_metadata(&input, role);
    deterministic_assessment(metadata.complexity)
}

fn deterministic_assessment(
    complexity: runtime::RouteTaskComplexity,
) -> TurnProbeAssessment {
    TurnProbeAssessment {
        complexity,
        intent: runtime::RouteTaskIntent::Other,
        provenance: runtime::RouteAssessmentProvenance::Deterministic,
    }
}

/// Say what the probe gate answered, to both ledgers that record it: the
/// process's attestation counters and the workspace's own durable row.
///
/// One place knows there are two sinks. Before this, each gate spelled its
/// reason as a string literal beside its own `attest_` call, which is how the
/// reason reached a ledger that dies with the process and no further — the
/// question "why did the judgment never run here?" had no answer a day later.
///
/// `SettingsUnavailable` is the one that escalates: a cost gate and a user's
/// own setting declining are normal operation, but a settings file that
/// cannot be read is nobody's choice.
fn note_gate(gate: ProbeGate) {
    match gate {
        ProbeGate::SettingsUnavailable => {
            telemetry::attest_failed(telemetry::HarnessFeature::RoutingProbe, gate.token());
        }
        ProbeGate::Admitted => {}
        ProbeGate::NotWorthIt | ProbeGate::ClassifierOff | ProbeGate::VerdictUnread => {
            telemetry::attest_declined(telemetry::HarnessFeature::RoutingProbe, gate.token());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        super::probe_gate::note(&cwd, gate);
    }
}

/// [`assess_turn_complexity`], but a turn the keyword tables read as *easy*
/// gets a second opinion from the model before that reading is allowed to
/// spend less effort on it.
///
/// The keyword classifier is systematically wrong in one direction that
/// matters: it scores by verb and length, so a short ask with a large,
/// open-ended deliverable — "랜딩페이지 디자인 만들어줘", "design a modern
/// dashboard UI" — lands in `Small` exactly like "이 설정값이 뭔지 알려줘"
/// does. That table reading used to lower the Smart floor by itself; now the
/// second opinion is what authorizes any downshift and owns the deliverable
/// intent. No keyword list fixes this: the signal is what the deliverable is,
/// not which words the request happens to use.
///
/// So the probe runs where a wrong "easy" costs something — `Trivial`/`Small`,
/// the bands it may authorize a lower floor for — plus the `Medium` turns
/// admitted for their intent read alone (see `probe_admission`), which may
/// not spend less than the tables already decided. An ordinary verb-matched
/// `Medium`, `Large`, and `Unknown` are returned untouched, paying nothing.
/// Fusion stays bounded by
/// [`runtime::fuse_probe_assessment`] (±1 band, risk only rises, low
/// confidence discarded), and every failure path — Smart routing off,
/// classifier off, no inventory, timeout, malformed JSON — falls back to the
/// deterministic verdict, so this can only ever refine it.
///
/// Unlike the sub-agent spawn path, this does NOT require
/// `classifier = probed`, and effort mode is intentionally not a gate. The
/// default `Deterministic` classifier still gets the second opinion here;
/// only an explicit [`RouteAutoClassifierMode::Off`] declines it.
///
/// Blocking (one bounded Fast-tier round-trip, memoized per task text). Call
/// it off the UI path — the TUI turn wraps it in `spawn_blocking`.
#[must_use]
pub fn assess_turn_complexity_probed(
    user_text: &str,
    parent_model: &str,
) -> runtime::RouteTaskComplexity {
    // No attempt to bill: this entry point is the deterministic-complexity
    // helper, not a turn's own classification.
    assess_turn_probed(user_text, parent_model, AssessmentReaders::default(), "").complexity
}

/// A probed turn assessment: the fused difficulty band plus what the turn asks
/// the model to PRODUCE.
///
/// The intent axis is why this exists next to
/// [`assess_turn_complexity_probed`]: the probe already reads the turn, and
/// the model's own read costs nothing extra on the wire. Provider-free and
/// failed-probe paths retain the neutral `Other` fallback; a probe that clears
/// its confidence gate remains authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnProbeAssessment {
    pub complexity: runtime::RouteTaskComplexity,
    pub intent: runtime::RouteTaskIntent,
    /// Whether the result remains keyword-only or includes a probe verdict
    /// that cleared fusion's confidence gate.
    pub provenance: runtime::RouteAssessmentProvenance,
}

/// What a probe verdict is allowed to replace for this turn.
///
/// Admission and AUTHORITY are two questions, and conflating them is what let
/// the design carve-out below hand over an axis it never meant to buy: a turn
/// admitted for its intent read arrived with the complexity axis delegated too,
/// because reaching `probe_exec` was the only thing that set
/// `provenance = TrustedProbe` — the sole key to the Smart effort band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeAdmission {
    /// Not worth a round-trip; the table verdict stands unexamined.
    Declined,
    /// The probe owns BOTH axes. An easy-band table verdict is exactly what a
    /// second opinion exists to revise, in either direction.
    FullVerdict,
    /// The probe owns the INTENT read; the deterministic band stands unless the
    /// probe RAISES it. See [`resolve_probed_complexity`].
    IntentOnly,
}

/// Who would read the probe's verdict this turn. Both axes it refines —
/// the fused band and the deliverable intent — have exactly two consumers:
/// the deep-gate verify leg (proportional VERIFY depth from the band, lens
/// from the intent; armed by the opt-in reactive gate or an installed gate)
/// and the exec contract (`exec_impl_model`, armed only under the Architect
/// deep-tier-only settings, asked of the settings at the call site; it reads
/// the band for plan-first/swap arming and the intent for eligibility).
/// Nothing lowers an effort floor from the band any more. A turn with
/// neither reader gains nothing from the probe and must not hold its first
/// request for a Fast-tier round-trip (r50: ~0.65 s of first-token latency
/// on every probed prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AssessmentReaders {
    /// A deep gate is armed for this turn (opt-in verify, or one already
    /// installed), so its VERIFY leg will read the band and the intent.
    pub verify_leg: bool,
}

/// Whether anyone reads the verdict this turn: an armed deep gate, or an exec
/// contract the settings arm for this main model.
#[must_use]
fn assessment_has_a_reader(readers: AssessmentReaders, exec_contract_armed: bool) -> bool {
    readers.verify_leg || exec_contract_armed
}

/// Any admission is worth its round-trip only when someone reads the verdict.
#[must_use]
fn admission_for_readers(admission: ProbeAdmission, verdict_read: bool) -> ProbeAdmission {
    if verdict_read {
        admission
    } else {
        ProbeAdmission::Declined
    }
}

/// Whether the keyword verdict for this turn is worth a probe round-trip, and
/// how much of it the probe may then replace.
///
/// Three classes qualify:
/// - `Trivial`/`Small` — the candidate bands a trusted probe may move DOWN, so
///   the table verdict needs a second opinion before it can spend less. This is
///   the only class where the probe owns the complexity axis
///   ([`ProbeAdmission::FullVerdict`]).
/// - `Medium` reached by LENGTH ALONE (no implementation verb matched, the
///   `>= 800` character rule). That is where a long design brief lands: it
///   never says "구현"/"fix", it is simply long. Its band is already the
///   default heavy one, so the probe is not buying a floor here — it is
///   buying the INTENT read, which nothing else can supply.
/// - A verb-matched `Medium` that MENTIONS design. See below.
///
/// An ordinary verb-matched `Medium` ("fix the auth module") is the common case
/// and stays excluded: its band is right, its intent is obvious, and probing it
/// would put a round-trip on the majority of turns.
///
/// The design carve-out exists because `Design` intent is probe-owned
/// EXCLUSIVELY — [`deterministic_assessment`] hard-codes `Other` — while the
/// implementation-verb table matches the ordinary way anyone asks for a design
/// change: "이 화면 디자인 수정해줘" (수정), "fix the spacing and redesign the
/// hero" (fix). Every such turn was declined as `not_worth_it`, so intent
/// stayed `Other` and all three Design consumers went dark together: no
/// `[zo:design-guidance]` reminder, no design effort ceiling, no design target.
/// The axis could only arm for a design brief phrased so as to name no verb at
/// all — which is not how the request is normally written.
///
/// This is an ADMISSION heuristic, not an intent verdict: a keyword cannot say
/// what a turn wants produced (the reason intent is probe-owned in the first
/// place), but it can say the question is worth asking. The probe still owns
/// the answer and fusion still gates it on confidence, so a false positive
/// costs one bounded round-trip — and, because both `Medium` classes admit as
/// [`ProbeAdmission::IntentOnly`], nothing else — while the false negative it
/// replaces cost the whole capability.
///
/// It asks [`mentions_design`], NOT whether the inferred role is
/// `RouteRole::Design`. The role ladder is a priority ordering in which write
/// intent outranks design, so "디자인 수정해줘" resolves to `Coding` — the
/// same collapse one level down, and reading the role here would have closed
/// the gate on exactly the turns it was opened for. Both questions read one
/// keyword list, so neither can drift from the other.
fn probe_admission(
    complexity: runtime::RouteTaskComplexity,
    input: &TaskMetadataInput<'_>,
    user_text: &str,
) -> ProbeAdmission {
    match complexity {
        runtime::RouteTaskComplexity::Trivial | runtime::RouteTaskComplexity::Small => {
            ProbeAdmission::FullVerdict
        }
        runtime::RouteTaskComplexity::Medium
            if !task_complexity_verb_matched(input) || mentions_design(None, "", user_text) =>
        {
            ProbeAdmission::IntentOnly
        }
        runtime::RouteTaskComplexity::Medium
        | runtime::RouteTaskComplexity::Large
        | runtime::RouteTaskComplexity::Unknown => ProbeAdmission::Declined,
    }
}

/// The complexity a probed turn carries out of fusion, honoring what admission
/// actually delegated.
///
/// [`ProbeAdmission::IntentOnly`] keeps the deterministic band against a
/// DOWNGRADE and takes a raise unchanged. Both `Medium` admissions say the same
/// thing in [`probe_admission`]'s doc — the band is already the heavy default,
/// so the round-trip buys the intent read — but until the clamp existed, saying
/// it was not enough: admission alone flipped `provenance` to `TrustedProbe`,
/// and a High-confidence `{small}` verdict then demoted `Medium` → `Small`,
/// which is the difference between Smart's default `xhigh..=max` and
/// `medium..=xhigh` (`smart_turn_effort_band_for_complexity`). A turn reading
/// "fix the flaky retry test in the design-tokens provider" would have bought
/// two rungs less reasoning off an incidental substring in a crate name.
///
/// A RAISE stays admissible because it costs nothing this gate is protecting:
/// `Medium` → `Large` is broad-scope evidence the deterministic tables missed,
/// and the band it selects rises. The clamp is asymmetric on purpose — the
/// hazard is spending less on work the tables already called ordinary.
///
/// Intent, risk, and provenance are untouched: the probe is authoritative on
/// what the turn wants produced, and that is what admission paid for.
fn resolve_probed_complexity(
    admission: ProbeAdmission,
    deterministic: runtime::RouteTaskComplexity,
    fusion: runtime::ProbeFusion,
) -> runtime::RouteTaskComplexity {
    if admission == ProbeAdmission::IntentOnly
        && fusion.effect == runtime::ProbeFusionEffect::LoweredComplexity
    {
        return deterministic;
    }
    fusion.complexity
}

/// [`assess_turn_complexity_probed`] plus the probe's intent read. Same gates,
/// same fail-open contract: every failure path — Smart routing off, classifier
/// off, no inventory, timeout, malformed JSON, or a probe below the confidence
/// gate — returns the deterministic complexity and neutral `Other` fallback. A
/// trusted probe replaces that seed verbatim on the axes its admission bought
/// (`resolve_probed_complexity`).
#[must_use]
pub fn assess_turn_probed(
    user_text: &str,
    parent_model: &str,
    readers: AssessmentReaders,
    attempt: &str,
) -> TurnProbeAssessment {
    let role = infer_route_role(None, "", user_text);
    let input = TaskMetadataInput::new(None, "", user_text);
    let metadata = classify_task_metadata(&input, role);
    let deterministic = deterministic_assessment(metadata.complexity);
    // The three gates below return the deterministic verdict without ever
    // reaching `probe_exec`, so nothing downstream would record them. Attesting
    // them here is what turns the probe's admission rate into a standing
    // observation instead of something measured by hand once: a table showing
    // `not_worth_it` on every turn says the cost gate is the reason a
    // probe-owned axis never armed, which is not a conclusion the probe's own
    // success/failure counters can reach.
    //
    // Two of the three are DECLINES — a cost gate and a user setting choosing
    // not to probe are normal operation and must never read as a defect. An
    // unreadable settings file is the odd one out: nothing chose that, so it
    // escalates as a failure.
    let admission = probe_admission(metadata.complexity, &input, user_text);
    if admission == ProbeAdmission::Declined {
        note_gate(ProbeGate::NotWorthIt);
        return deterministic;
    }
    let Some(settings) = super::settings::read_smart_runtime_settings() else {
        note_gate(ProbeGate::SettingsUnavailable);
        return deterministic;
    };
    if !settings.enabled || settings.auto_classifier == RouteAutoClassifierMode::Off {
        note_gate(ProbeGate::ClassifierOff);
        return deterministic;
    }
    // The verdict is read by the deep-gate verify leg and the exec contract
    // only; with neither armed the round-trip is pure first-token latency. A
    // decline, attested like the cost gate above.
    let verdict_read = assessment_has_a_reader(
        readers,
        super::settings::exec_impl_model_armed(parent_model, &settings),
    );
    let admission = admission_for_readers(admission, verdict_read);
    if admission == ProbeAdmission::Declined {
        note_gate(ProbeGate::VerdictUnread);
        return deterministic;
    }
    note_gate(ProbeGate::Admitted);
    let inventory = runtime::connected_model_inventory(parent_model);
    let Some(probe) =
        super::probe_exec::route_probe_assessment(&inventory, parent_model, "", user_text, attempt)
    else {
        return deterministic;
    };
    let fusion = runtime::fuse_probe_assessment(
        metadata.complexity,
        metadata.risk,
        deterministic.intent,
        &probe,
    );
    TurnProbeAssessment {
        complexity: resolve_probed_complexity(admission, metadata.complexity, fusion),
        intent: fusion.intent,
        provenance: fusion.provenance,
    }
}

/// Whether a whole turn's text carries concrete implementation/write intent,
/// using the SAME multilingual keyword classifier the per-agent router's
/// write-intent gate uses (`task_has_write_intent`). The host's Architect
/// contract consults this to decide whether a turn is implementation-shaped
/// (EXEC legs swap to the implementer client) — pure and deterministic, safe
/// for the per-turn host path.
#[must_use]
pub fn turn_has_write_intent(user_text: &str) -> bool {
    super::infer::task_has_write_intent("", user_text)
}

/// Complexity + write-intent of a DELEGATED agent task (its `description` +
/// `prompt` slice), distinct from the whole-turn assessment. The guard needs
/// the slice's own difficulty/effect, not the user turn's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentTaskAssessment {
    pub complexity: runtime::RouteTaskComplexity,
    pub has_write_intent: bool,
}

/// Classify a delegated agent task from the EXECUTABLE task text — the spawn's
/// `prompt`, which is what the child actually runs (`AgentJob::prompt`) — using
/// the SAME corpus-pinned role/metadata and write-intent classifiers as the
/// whole-turn helpers. Deliberately ignores `description`, `name`,
/// `subagent_type`, and model/route fields: those are model-authored labels the
/// child never executes, so classifying them would let a spawn dodge the guard
/// (e.g. an inflated `description`) without changing the real task. Pure and
/// deterministic.
#[must_use]
pub fn assess_agent_task(prompt: &str) -> AgentTaskAssessment {
    let role = infer_route_role(None, "", prompt);
    let complexity =
        classify_task_metadata(&TaskMetadataInput::new(None, "", prompt), role).complexity;
    let has_write_intent = super::infer::task_has_write_intent("", prompt);
    AgentTaskAssessment {
        complexity,
        has_write_intent,
    }
}

/// Assess a turn's orchestration shape from smart-routing evidence (metadata
/// classifier + need planner + shape selector), reusing the exact pipeline the
/// per-agent router uses so the host and model layers agree on the taxonomy.
#[must_use]
pub fn assess_turn_orchestration(user_text: &str) -> TurnOrchestrationHint {
    let role = infer_route_role(None, "", user_text);
    let metadata = classify_task_metadata(&TaskMetadataInput::new(None, "", user_text), role);
    let needs = plan_agent_needs(&metadata);
    // The turn's text goes in as the DESCRIPTION, not as a brief: a brief is
    // what a parent hands a child (and may quote the person inside it, which
    // is why `requested_shape_haystack` refuses to read one), while these are
    // the person's own words about their own turn. Nothing else moves — the
    // evidence haystack holds the same text either way.
    let evidence = infer_route_shape_evidence(&RouteEvidenceInput {
        subagent_type: None,
        name: None,
        description: user_text,
        prompt: "",
        workflow_member: false,
        fanout_position: None,
        auto_classifier: RouteAutoClassifierMode::Deterministic,
    });
    // The user's explicitly requested shape (from the evidence pipeline) is
    // discarded by `select_route_shape`, which returns the natural shape — so
    // capture it here, whole.
    let decision = select_route_shape(&shape_input_with_evidence(&metadata, &needs, &evidence));
    TurnOrchestrationHint {
        shape: decision.shape,
        need_count: needs.len(),
        confidence: decision.confidence,
        risk: metadata.risk,
        requested_shape: evidence.requested_shape,
        user_named_model: user_named_model(user_text),
    }
}

/// What the HOST does with a turn before the model sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPrelude {
    /// Nothing: the model decides every spawn from its own delegation rubric.
    ModelLed,
    /// Split the turn into up to `width` independent read-only analyses, run
    /// them in parallel, and seat their findings in the turn's context before
    /// the model turn starts.
    Fanout { width: usize },
}

/// The host's orchestration decision for a turn, from its difficulty and its
/// shape.
///
/// A difficulty LADDER, not a keyword table: `Trivial`/`Small` never split
/// (coordination would cost more than the split saves), `Medium` stays
/// model-led (the model plans first and delegates what it must), `Large` is
/// broad enough that a parallel pre-analysis can pay for itself — but only
/// when the shape evidence already shows independent lanes. A broad turn
/// whose independence is unknown (a coupled migration, a serial change)
/// stays model-led until the model has a concrete plan; difficulty alone
/// used to buy every `Large` turn a planner call before the first main token
/// (t-3854). The width comes from the same projection the model-invoked
/// fan-out uses ([`fanout_width_for`]), so the host and the model never
/// disagree about how wide a difficulty is.
///
/// The person's own shape outranks the ladder in both directions: a request
/// for lanes is honored at the narrowest real split even where the ladder
/// would refuse; a request for any OTHER shape — one specialist, a sequence,
/// a repair loop — is a delegation the model leads, never a host split; and a
/// request for NO delegation wins over everything.
///
/// `nested` wins over all of it: a teammate's turn IS a delegation already —
/// its parent split the work and handed it this brief — so the host never
/// pre-analyses a child's brief. A brief that read as broad used to spawn a
/// decomposition helper, whose brief read as broad, whose … ("무한 스폰",
/// 2026-09-07).
#[must_use]
pub fn decide_host_prelude(
    policy: HostOrchestration,
    assessment: TurnProbeAssessment,
    hint: TurnOrchestrationHint,
    nested: bool,
) -> HostPrelude {
    if nested || !matches!(policy, HostOrchestration::Auto) {
        return HostPrelude::ModelLed;
    }
    let ladder_width = fanout_width_for(assessment.complexity);
    match hint.requested_shape {
        Some(requested) if shape_runs_lanes(requested) => HostPrelude::Fanout {
            width: ladder_width.unwrap_or(MIN_FANOUT_SUBTASKS),
        },
        Some(_) => HostPrelude::ModelLed,
        None => match (assessment.complexity, ladder_width) {
            (RouteTaskComplexity::Large, Some(width)) if shape_runs_lanes(hint.shape) => {
                HostPrelude::Fanout { width }
            }
            _ => HostPrelude::ModelLed,
        },
    }
}

#[cfg(test)]
mod host_prelude_tests {
    use super::{
        assess_turn_deterministic, assess_turn_orchestration, decide_host_prelude, user_named_model,
        HostOrchestration, HostPrelude, TurnOrchestrationHint, TurnProbeAssessment,
    };
    use crate::fanout::{MAX_FANOUT_SUBTASKS, MIN_FANOUT_SUBTASKS};
    use runtime::{RouteAssessmentProvenance, RouteShapeKind, RouteTaskComplexity as C, RouteTaskIntent};

    /// 「fable 검토 받아」 — the person named a model in their own words, and
    /// that name is a registry alias. It must come out as the canonical id so
    /// the spawn can pin it (live report 2026-09-09: the parent read "Fable"
    /// as a persona and spawned the reviewer on the default Opus).
    #[test]
    fn a_model_the_person_names_in_the_turn_is_read_as_its_canonical_id() {
        assert_eq!(
            user_named_model("제안한 방법으로 구현 계획 세워주고 fable 검토 받아"),
            Some("claude-fable-5-1")
        );
        // Korean particles glue onto the word; the boundary is the script change.
        assert_eq!(user_named_model("opus로 리뷰해줘"), Some("claude-opus-5"));
        assert_eq!(user_named_model("Sonnet 으로 돌려"), Some("claude-sonnet-5"));
        assert_eq!(user_named_model("use claude-fable-5-1 for this"), Some("claude-fable-5-1"));
    }

    /// No near-miss snapping in the person's prose: `table` is a table, not
    /// Fable. And two different models named at once is not a pin — the
    /// assistant's choice stands.
    #[test]
    fn prose_that_names_no_single_model_pins_nothing() {
        assert_eq!(user_named_model("table을 만들어 줘"), None);
        assert_eq!(user_named_model("이 파일 검토해줘"), None);
        assert_eq!(user_named_model("opus가 짠 코드를 fable로 검토해"), None);
        assert_eq!(user_named_model(""), None);
    }

    /// The hint carries the name so the spawn door can read it as a person pin.
    #[test]
    fn the_turn_hint_carries_the_person_named_model() {
        let hint = assess_turn_orchestration("구현 계획 세우고 fable 검토 받아");
        assert_eq!(hint.user_named_model, Some("claude-fable-5-1"));
        assert_eq!(assess_turn_orchestration("검토해줘").user_named_model, None);
    }

    /// A child's turn is already a delegation: even a broad brief that asks
    /// for parallel work stays model-led inside a teammate. The root beside it
    /// keeps the ladder's answer.
    #[test]
    fn a_nested_session_never_runs_the_host_prelude() {
        let hint = assess_turn_orchestration("split this into parallel lanes");
        assert_eq!(hint.requested_shape, Some(RouteShapeKind::ParallelLanes));
        assert_eq!(
            decide_host_prelude(HostOrchestration::Auto, assessment(C::Large), hint, true),
            HostPrelude::ModelLed
        );
        assert!(matches!(
            decide_host_prelude(HostOrchestration::Auto, assessment(C::Large), hint, false),
            HostPrelude::Fanout { .. }
        ));
    }

    fn assessment(complexity: C) -> TurnProbeAssessment {
        TurnProbeAssessment {
            complexity,
            intent: RouteTaskIntent::Other,
            provenance: RouteAssessmentProvenance::Deterministic,
        }
    }

    fn plain_hint() -> TurnOrchestrationHint {
        assess_turn_orchestration("hello there")
    }

    /// A hint whose shape evidence found independent lanes, with no shape the
    /// person asked for.
    fn lanes_hint() -> TurnOrchestrationHint {
        TurnOrchestrationHint {
            shape: RouteShapeKind::ParallelLanes,
            ..plain_hint()
        }
    }

    #[test]
    fn the_ladder_splits_large_and_only_large() {
        for (complexity, expected) in [
            (C::Trivial, HostPrelude::ModelLed),
            (C::Small, HostPrelude::ModelLed),
            (C::Medium, HostPrelude::ModelLed),
            (C::Unknown, HostPrelude::ModelLed),
            (C::Large, HostPrelude::Fanout { width: MAX_FANOUT_SUBTASKS }),
        ] {
            assert_eq!(
                decide_host_prelude(HostOrchestration::Auto, assessment(complexity), lanes_hint(), false),
                expected,
                "{complexity:?}"
            );
        }
    }

    /// Difficulty alone does not buy a planner call: a `Large` turn whose
    /// evidence shows no independent lanes (a single serial change, a coupled
    /// sequence, one bounded specialist need) stays model-led — the planner
    /// call used to delay the first main token on every such turn (t-3854).
    #[test]
    fn a_large_turn_without_independent_lanes_stays_model_led() {
        for shape in [
            RouteShapeKind::Solo,
            RouteShapeKind::OneSpecialist,
            RouteShapeKind::SequentialWorkflow,
            RouteShapeKind::RepairLoop,
        ] {
            let hint = TurnOrchestrationHint { shape, ..plain_hint() };
            assert_eq!(
                decide_host_prelude(HostOrchestration::Auto, assessment(C::Large), hint, false),
                HostPrelude::ModelLed,
                "{shape:?}"
            );
        }
        assert_eq!(
            decide_host_prelude(
                HostOrchestration::Auto,
                assessment(C::Large),
                TurnOrchestrationHint { shape: RouteShapeKind::ParallelRepairLoop, ..plain_hint() },
                false
            ),
            HostPrelude::Fanout { width: MAX_FANOUT_SUBTASKS },
            "independent lanes with findings are still lanes"
        );
    }

    /// The person's shape is kept whole: "one specialist" is a delegation the
    /// model leads, never a host split — at any difficulty. Only a request for
    /// lanes earns the host's parallel pre-analysis.
    #[test]
    fn a_requested_non_lane_shape_is_model_led_at_any_difficulty() {
        let specialist = assess_turn_orchestration("one specialist is enough");
        assert_eq!(specialist.requested_shape, Some(RouteShapeKind::OneSpecialist));
        assert!(specialist.user_requested_delegation(), "the spawn guard still reads it as delegation");
        for complexity in [C::Small, C::Medium, C::Large] {
            assert_eq!(
                decide_host_prelude(HostOrchestration::Auto, assessment(complexity), specialist, false),
                HostPrelude::ModelLed,
                "one specialist at {complexity:?}"
            );
        }
        for requested in [RouteShapeKind::SequentialWorkflow, RouteShapeKind::RepairLoop] {
            let hint = TurnOrchestrationHint {
                requested_shape: Some(requested),
                ..lanes_hint()
            };
            assert_eq!(
                decide_host_prelude(HostOrchestration::Auto, assessment(C::Large), hint, false),
                HostPrelude::ModelLed,
                "the person's {requested:?} outranks lane evidence"
            );
        }
        let parallel_repair = TurnOrchestrationHint {
            requested_shape: Some(RouteShapeKind::ParallelRepairLoop),
            ..plain_hint()
        };
        assert_eq!(
            decide_host_prelude(HostOrchestration::Auto, assessment(C::Small), parallel_repair, false),
            HostPrelude::Fanout { width: MIN_FANOUT_SUBTASKS }
        );
    }

    #[test]
    fn only_the_auto_policy_lets_the_host_spawn() {
        for policy in [HostOrchestration::ModelLed, HostOrchestration::Off] {
            assert_eq!(
                decide_host_prelude(policy, assessment(C::Large), plain_hint(), false),
                HostPrelude::ModelLed
            );
        }
    }

    #[test]
    fn the_user_outranks_the_ladder_in_both_directions() {
        let parallel = assess_turn_orchestration("compare these two parsers in parallel");
        assert!(parallel.user_requested_delegation());
        assert_eq!(
            decide_host_prelude(HostOrchestration::Auto, assessment(C::Small), parallel, false),
            HostPrelude::Fanout { width: MIN_FANOUT_SUBTASKS },
            "an explicit parallel ask gets the narrowest real split"
        );
        let solo = assess_turn_orchestration("migrate every module in the whole repo, solo, no agents");
        assert!(solo.user_requested_solo());
        assert_eq!(
            decide_host_prelude(HostOrchestration::Auto, assessment(C::Large), solo, false),
            HostPrelude::ModelLed
        );
        let solo_over_lanes = TurnOrchestrationHint {
            requested_shape: Some(RouteShapeKind::Solo),
            ..lanes_hint()
        };
        assert_eq!(
            decide_host_prelude(HostOrchestration::Auto, assessment(C::Large), solo_over_lanes, false),
            HostPrelude::ModelLed,
            "an explicit solo outranks lane evidence"
        );
    }

    /// The prompt the hermetic e2e drives: pinned here so the screen test and
    /// the classifier can never drift apart on what counts as `Large` with
    /// independent lanes (two slash-joined modules).
    #[test]
    fn a_whole_repo_migration_across_named_lanes_classifies_large_and_fans_out() {
        let prompt = "레포 전체를 훑어서 parser/lexer 모듈을 모두 마이그레이션해줘. 여러 단계에 걸쳐 진행해야 한다.";
        let assessment = assess_turn_deterministic(prompt);
        assert_eq!(assessment.complexity, C::Large);
        let hint = assess_turn_orchestration(prompt);
        assert_eq!(hint.requested_shape, None, "the person asked for no shape");
        assert_eq!(hint.shape, RouteShapeKind::ParallelLanes);
        assert_eq!(
            decide_host_prelude(HostOrchestration::Auto, assessment, hint, false),
            HostPrelude::Fanout { width: MAX_FANOUT_SUBTASKS }
        );
    }

    /// The same migration with no independent lanes in evidence is still
    /// `Large`, and now reaches the main model without a planner call first.
    #[test]
    fn a_whole_repo_migration_without_lanes_is_large_but_model_led() {
        let prompt = "레포 전체를 훑어서 모든 모듈을 마이그레이션해줘. 여러 단계에 걸쳐 진행해야 한다.";
        let assessment = assess_turn_deterministic(prompt);
        assert_eq!(assessment.complexity, C::Large);
        let hint = assess_turn_orchestration(prompt);
        assert_eq!(hint.requested_shape, None);
        assert_ne!(hint.shape, RouteShapeKind::ParallelLanes);
        assert_eq!(
            decide_host_prelude(HostOrchestration::Auto, assessment, hint, false),
            HostPrelude::ModelLed
        );
    }
}

#[cfg(test)]
mod probe_gate_tests {
    use super::{
        admission_for_readers, assessment_has_a_reader, probe_admission, resolve_probed_complexity,
        AssessmentReaders, ProbeAdmission, TaskMetadataInput,
    };
    use runtime::RouteTaskComplexity as C;

    fn admission(user_text: &str, complexity: C) -> ProbeAdmission {
        probe_admission(complexity, &TaskMetadataInput::new(None, "", user_text), user_text)
    }

    fn gate(user_text: &str, complexity: C) -> bool {
        admission(user_text, complexity) != ProbeAdmission::Declined
    }

    /// The fusion a High-confidence probe verdict of `probe` produces against a
    /// deterministic reading of `table`. Built through the real
    /// [`runtime::fuse_probe_assessment`] so these tests move with fusion's own
    /// bounds rather than restating them.
    fn fusion_of(table: C, probe: C) -> runtime::ProbeFusion {
        runtime::fuse_probe_assessment(
            table,
            runtime::RouteTaskRisk::Unknown,
            runtime::RouteTaskIntent::Other,
            &runtime::ProbeAssessment {
                complexity: probe,
                risk: runtime::RouteTaskRisk::Unknown,
                confidence: runtime::RouteConfidence::High,
                intent: runtime::RouteTaskIntent::Design,
            },
        )
    }

    #[test]
    fn the_easy_bands_still_probe() {
        for complexity in [C::Trivial, C::Small] {
            assert!(
                gate("이 설정값이 뭔지 알려줘", complexity),
                "{complexity:?} needs a second opinion before it may lower effort"
            );
        }
    }

    #[test]
    fn a_length_promoted_medium_probes_but_a_verb_matched_one_does_not() {
        // No implementation verb anywhere — this brief is Medium purely because
        // it is long. That is where a real design brief lands, and it is the
        // only signal that says so.
        let long_design_brief = "우리 회사 랜딩페이지를 새로 만들어줘. ".repeat(60);
        assert!(
            long_design_brief.chars().count() >= 800,
            "the fixture must reach the long-brief rule"
        );
        assert!(gate(&long_design_brief, C::Medium));

        // An ordinary implementation turn: the verb table fired, the band is
        // already right, and probing it would tax the majority of turns.
        assert!(!gate("이 함수의 버그를 수정해줘", C::Medium));
        assert!(!gate("implement the retry path", C::Medium));
    }

    #[test]
    fn a_design_ask_phrased_with_an_implementation_verb_still_probes() {
        // The gap this closes: `Design` intent is probe-owned exclusively, and
        // the implementation-verb table matches the ordinary way anyone asks
        // for a design change. Every such turn was declined as `not_worth_it`,
        // so intent stayed `Other` and the whole Design axis — guidance
        // reminder, effort ceiling, design target — could never arm on the way
        // people actually write the request.
        for prompt in [
            "이 화면 디자인 수정해줘",
            "디자인 좀 고쳐줘",
            "fix the spacing and redesign the hero",
            "refactor the design of this dashboard",
            "프론트엔드 레이아웃 변경해줘",
        ] {
            assert!(
                gate(prompt, C::Medium),
                "a verb-matched design ask must still buy the intent read: {prompt}"
            );
        }
    }

    #[test]
    fn the_design_carve_out_reads_the_turn_not_the_role_ladder() {
        // The role ladder ranks write intent ABOVE design, so this turn's role
        // is `Coding` — gating on the role would have reproduced the very
        // collapse being fixed, one level down.
        assert_eq!(
            super::infer_route_role(None, "", "이 화면 디자인 수정해줘"),
            runtime::RouteRole::Coding,
            "premise: the ladder does not call this a design turn"
        );
        assert!(gate("이 화면 디자인 수정해줘", C::Medium));
    }

    #[test]
    fn the_design_carve_out_does_not_widen_the_gate_beyond_medium() {
        // It buys the INTENT read for a band that already has the right
        // complexity — it must not start paying for the heavy bands, whose
        // exclusion is about cost, not about intent.
        for complexity in [C::Large, C::Unknown] {
            assert!(!gate("이 화면 디자인 수정해줘", complexity));
        }
    }

    /// The verdict has exactly two readers — the deep-gate verify leg and the
    /// exec contract. A turn where neither is armed gains nothing from the
    /// probe, and the r50 bench paid one Fast-tier round-trip of first-token
    /// latency (~0.65 s live; 0.61 s on the harness with a 0.6 s probe) on
    /// every probed prompt to learn a verdict nobody read.
    #[test]
    fn a_probe_runs_only_when_something_reads_its_verdict() {
        let none = AssessmentReaders::default();
        let verify = AssessmentReaders { verify_leg: true };
        assert!(!assessment_has_a_reader(none, false));
        assert!(assessment_has_a_reader(verify, false));
        assert!(assessment_has_a_reader(none, true));
        assert_eq!(
            admission_for_readers(ProbeAdmission::IntentOnly, false),
            ProbeAdmission::Declined
        );
        assert_eq!(
            admission_for_readers(ProbeAdmission::IntentOnly, true),
            ProbeAdmission::IntentOnly
        );
        // The easy bands delegate complexity as well as intent — and the band
        // has the same two readers (the deep-gate verify depth and the exec
        // contract's plan-first/swap arming); nothing lowers an effort floor
        // from it any more. So without a reader the FullVerdict probe is
        // declined too: a short question does not wait a round-trip to learn
        // a band nobody reads.
        assert_eq!(
            admission_for_readers(ProbeAdmission::FullVerdict, false),
            ProbeAdmission::Declined
        );
        assert_eq!(
            admission_for_readers(ProbeAdmission::FullVerdict, true),
            ProbeAdmission::FullVerdict
        );
        assert_eq!(
            admission_for_readers(ProbeAdmission::Declined, true),
            ProbeAdmission::Declined
        );
    }

    /// The two r50 bench prompts whose first token trailed the rest by the
    /// probe's round-trip (tasks 13 and 15) read `Trivial` from the keyword
    /// table — the "docs" marker matched "docstring" / "the docs warn" — so
    /// they were easy-band `FullVerdict` probes. With failing-test evidence they
    /// read `Medium` with no complexity verb: intent-only admission, which the
    /// reader gate declines when nothing reads the intent. Pinned end to end.
    #[test]
    fn the_r50_probed_bench_prompts_are_medium_turns_the_reader_gate_leaves_unprobed() {
        for prompt in [
            "./run_tests.sh fails. Make parse_csv in csvlite.py obey every rule in its docstring. \
             Do not import Python's csv module — this package exists because we need our own parser.",
            "./run_tests.sh fails. report.py reaches for three standard-library calls in ways the \
             docs warn against; find them and make the tests pass without rewriting the module.",
        ] {
            let complexity = super::assess_turn_deterministic(prompt).complexity;
            assert_eq!(complexity, C::Medium, "{prompt}");
            assert_eq!(admission(prompt, complexity), ProbeAdmission::IntentOnly, "{prompt}");
            assert_eq!(
                admission_for_readers(admission(prompt, complexity), false),
                ProbeAdmission::Declined,
                "{prompt}"
            );
        }
        // A verb-matched Medium prompt (r50 task 11) never probed, before or after.
        let fix = "./run_tests.sh fails. The invoice totals are wrong; fix the rounding in money.py.";
        assert_eq!(super::assess_turn_deterministic(fix).complexity, C::Medium);
        assert_eq!(admission(fix, C::Medium), ProbeAdmission::Declined);
    }

    #[test]
    fn every_admitted_medium_delegates_the_intent_axis_alone() {
        // Both `Medium` classes exist to buy the intent read against a band
        // that is already the heavy default. Only the easy bands — the ones a
        // trusted probe may legitimately spend less on — delegate complexity.
        let long_design_brief = "우리 회사 랜딩페이지를 새로 만들어줘. ".repeat(60);
        for prompt in [
            "이 화면 디자인 수정해줘",
            "fix the flaky retry test in the design-tokens provider",
            long_design_brief.as_str(),
        ] {
            assert_eq!(admission(prompt, C::Medium), ProbeAdmission::IntentOnly);
        }
        for complexity in [C::Trivial, C::Small] {
            assert_eq!(
                admission("이 설정값이 뭔지 알려줘", complexity),
                ProbeAdmission::FullVerdict
            );
        }
    }

    #[test]
    fn an_intent_only_admission_refuses_a_probe_driven_downgrade() {
        // The defect this closes: admission alone flipped provenance to
        // `TrustedProbe`, and a High-confidence `{small}` verdict then demoted
        // Medium → Small — which is the difference between Smart's default
        // `xhigh..=max` band and `medium..=xhigh`. So "fix the flaky retry test
        // in the design-tokens provider" bought two rungs less reasoning off an
        // incidental substring in a crate name, while the byte-identical ask
        // naming `auth` instead kept the full band.
        let fusion = fusion_of(C::Medium, C::Small);
        assert_eq!(
            fusion.complexity,
            C::Small,
            "premise: fusion itself does allow this demotion"
        );
        assert_eq!(
            resolve_probed_complexity(ProbeAdmission::IntentOnly, C::Medium, fusion),
            C::Medium
        );
        assert_eq!(
            fusion.intent,
            runtime::RouteTaskIntent::Design,
            "the intent read admission actually paid for still arrives"
        );
    }

    #[test]
    fn an_intent_only_admission_still_takes_a_raise() {
        // The clamp is asymmetric on purpose: a raise is broad-scope evidence
        // the tables missed, and the band it selects rises. The hazard being
        // guarded is spending LESS on work already called ordinary.
        let fusion = fusion_of(C::Medium, C::Large);
        assert_eq!(
            resolve_probed_complexity(ProbeAdmission::IntentOnly, C::Medium, fusion),
            C::Large
        );
    }

    #[test]
    fn a_full_verdict_admission_keeps_the_probes_complexity() {
        // `Trivial`/`Small` are admitted precisely so a second opinion can move
        // the band; clamping there would defeat the reason the gate opens.
        let fusion = fusion_of(C::Small, C::Trivial);
        assert_eq!(
            resolve_probed_complexity(ProbeAdmission::FullVerdict, C::Small, fusion),
            C::Trivial
        );
    }

    #[test]
    fn the_heavy_and_unknown_bands_never_pay() {
        for complexity in [C::Large, C::Unknown] {
            assert!(!gate("레포 전체를 마이그레이션해줘", complexity));
        }
    }

    #[test]
    fn a_verb_matched_medium_returns_the_neutral_deterministic_verdict() {
        // Gated out before any settings read, inventory load, or round-trip, so
        // this is provider-free and the fail-open contract is observable.
        let assessment = super::assess_turn_probed(
            "이 함수의 버그를 수정해줘",
            "claude-opus-5",
            super::AssessmentReaders::default(),
            "",
        );
        assert_eq!(assessment.complexity, C::Medium);
        assert_eq!(assessment.intent, runtime::RouteTaskIntent::Other);
        assert_eq!(
            assessment.provenance,
            runtime::RouteAssessmentProvenance::Deterministic
        );
    }

    #[test]
    fn a_large_design_turn_outside_the_probe_gate_stays_keyword_neutral() {
        let assessment = super::assess_turn_probed(
            "design and build a landing page across the whole repo",
            "claude-opus-5",
            super::AssessmentReaders::default(),
            "",
        );
        assert_eq!(assessment.complexity, C::Large);
        assert_eq!(assessment.intent, runtime::RouteTaskIntent::Other);
        assert_eq!(
            assessment.provenance,
            runtime::RouteAssessmentProvenance::Deterministic
        );
    }

    #[test]
    fn deterministic_assessment_never_assigns_task_intent() {
        let prompts = [
            "랜딩 페이지 디자인 만들어줘",
            "Improve the design",
            "디자인 제작해줘",
            "Redesign the hero",
            "Create a polished pricing page",
            "Give the dashboard a cleaner visual hierarchy",
            "Make this checkout screen feel premium",
            "이 화면을 더 세련되게 디자인해줘",
            "모바일 앱 온보딩 화면을 만들어줘",
            "히어로 섹션을 새롭게 디자인해줘",
            "버튼과 타이포그래피를 전면 재디자인해줘",
            "Design and build a visually striking homepage for NOVA",
            "Design a responsive portfolio page",
            "implement the retry backoff",
            "이 함수의 버그를 수정해줘",
            "reproduce the flaky failure",
            "이 버그 재현해줘",
            "review the authentication changes",
            "verify the migration plan",
            "what does this flag do",
            "Explain the design of this trait",
            "Review the database schema design",
        ];
        for prompt in prompts {
            assert_eq!(
                super::assess_turn_deterministic(prompt).intent,
                runtime::RouteTaskIntent::Other,
                "{prompt:?}: deterministic keyword roles must not assign intent"
            );
        }
    }

    /// The verbatim brief from the CC-vs-zo landing-page benchmark. The old
    /// deterministic path armed `Design` only because two keyword classifiers
    /// happened to disagree about "build". Intent is now model-owned, so this
    /// provider-free seam must stay neutral at every effort.
    #[test]
    fn the_benchmark_landing_page_brief_does_not_arm_design_deterministically() {
        let assessment = super::assess_turn_deterministic(
            "Design and build a visually striking single-page landing page for \
             \"NOVA\", a fictional private spaceflight company offering commercial \
             journeys around the Moon.",
        );
        assert_eq!(
            assessment.intent,
            runtime::RouteTaskIntent::Other,
            "keyword text must not arm the model-owned design axis"
        );
        assert_eq!(
            assessment.provenance,
            runtime::RouteAssessmentProvenance::Deterministic
        );
    }
}
