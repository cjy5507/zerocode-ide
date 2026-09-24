//! Routing probe: a model-verbalized `{complexity, risk, confidence}`
//! self-assessment fused over the deterministic keyword classifier.
//!
//! The jacobian-lens principle behind this module: a cheap early readout of
//! what a model is "prepared to say" predicts the expensive final outcome —
//! here, a ~200-token Fast-tier classification call predicts whether a task
//! deserves a stronger tier, before any tier commits. This module is the pure
//! core only (parsing, fusion arithmetic, prompt text); the provider call
//! itself lives in the tools crate next to the other blocking client users.
//!
//! Fusion is deliberately bounded: the deterministic classifier always runs
//! and its verdict is the anchor. A probe may move complexity at most one
//! band, may only *raise* risk, and is discarded entirely below its
//! confidence gate — a hallucinated probe can therefore never swing a
//! trivial task to the Deep tier (or a repo-wide migration to a Fast model)
//! on its own.

use super::policy::{RouteConfidence, RouteTaskComplexity, RouteTaskRisk};

/// What the turn asks the model to PRODUCE — an axis orthogonal to difficulty.
/// The deterministic role classifier supplies a conservative fallback for
/// concrete write/review turns, while a trusted probe remains authoritative
/// when it can read the deliverable more precisely.
///
/// `Other` is the neutral value on purpose: every absent, malformed, or
/// unrecognized probe resolves to it, and every consumer treats `Other`
/// exactly as it behaved before this axis existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RouteTaskIntent {
    /// Visual/UI/UX work — a page, a component's appearance, a layout, a
    /// theme, a design system.
    Design,
    /// Non-visual code the turn wants written or changed.
    Implementation,
    /// Reading, explaining, investigating, reviewing — no deliverable artifact.
    Analysis,
    /// Anything else, and the fail-open value for a missing/unknown intent.
    #[default]
    Other,
}

impl RouteTaskIntent {
    /// Every intent — the one place the set is listed.
    pub const ALL: [RouteTaskIntent; 4] =
        [Self::Design, Self::Implementation, Self::Analysis, Self::Other];

    /// Lowercase wire token, for audit notes and the probe contract.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Design => "design",
            Self::Implementation => "implementation",
            Self::Analysis => "analysis",
            Self::Other => "other",
        }
    }

    /// The inverse of [`Self::as_str`]; `None` for an unknown string.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|intent| intent.as_str() == label)
    }
}

/// The version of the routing rubric's words: the axis table below, the probe
/// prompt rendered from it, the task text both readers are handed
/// ([`rubric_task_text`]), and the typed questions `decision.rs` asks from it.
///
/// Bump it whenever any of those words change. The decision shadow keys its
/// memo on it and stamps it on every ledger row, so judgments made under
/// different words never pool into one measurement. `decision.rs` pins the
/// rendered words to this number, so an edit that forgets the bump is a red
/// test rather than a quietly mixed ledger.
pub const DECISION_RUBRIC_VERSION: u32 = 1;

/// How many characters of a task a routing judgment reads — the probe's
/// prompt and the typed judgment's state alike. The head of a brief is what
/// bands it; the cap bounds what each call costs and what leaves the machine.
/// The number is the Jev use table's routing row (`zerocode_core::jev`), the
/// one place what each use sends is capped.
pub const RUBRIC_TASK_CHAR_CAP: usize = zerocode_core::jev::ROUTING_TASK_CHAR_CAP;

/// One axis of the routing rubric: what it asks, the closed set of answers it
/// offers, and what some of those answers mean.
///
/// Tokens are the router enums' own labels (`as_label`/`as_str`), so the words
/// a reader answers in are the words the route-outcome records already keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RubricAxis {
    /// The key the probe answers under, and the name a typed question is asked by.
    pub name: &'static str,
    /// What the axis measures, in the words every reader is given.
    pub question: &'static str,
    /// The answers the axis offers, lowest first when [`Self::ordered`].
    pub tokens: &'static [&'static str],
    /// What a token means, in the order the probe prompt reads them. A token
    /// with no entry is offered by name alone.
    pub descriptions: &'static [(&'static str, &'static str)],
    /// Whether the tokens form a scale, so an answer below the label is an
    /// under-estimate rather than just a different answer.
    pub ordered: bool,
    /// Whether the axis is the reader's report on itself rather than a
    /// judgment of the task. The probe states its confidence here; a typed
    /// judgment returns a whole distribution instead and is never asked it.
    pub self_report: bool,
}

impl RubricAxis {
    /// The position of `token` among the answers this axis offers.
    #[must_use]
    pub fn position(&self, token: &str) -> Option<usize> {
        self.tokens.iter().position(|offered| *offered == token)
    }

    /// What the rubric says `token` means, when it says anything.
    #[must_use]
    pub fn description(&self, token: &str) -> Option<&'static str> {
        self.descriptions
            .iter()
            .find_map(|(named, description)| (*named == token).then_some(*description))
    }
}

pub const COMPLEXITY_AXIS: RubricAxis = RubricAxis {
    name: zerocode_core::jev::questions::ROUTING_COMPLEXITY_ID,
    question: "how much reasoning/context the task needs end to end",
    tokens: &[
        RouteTaskComplexity::Trivial.as_label(),
        RouteTaskComplexity::Small.as_label(),
        RouteTaskComplexity::Medium.as_label(),
        RouteTaskComplexity::Large.as_label(),
    ],
    descriptions: &[
        (
            RouteTaskComplexity::Large.as_label(),
            "repo-wide, multi-subsystem, or architecturally hard",
        ),
        (RouteTaskComplexity::Trivial.as_label(), "a label/typo-class edit"),
    ],
    ordered: true,
    self_report: false,
};

pub const RISK_AXIS: RubricAxis = RubricAxis {
    name: zerocode_core::jev::questions::ROUTING_RISK_ID,
    question: "blast radius of a wrong edit (credentials, deletion, security = high+)",
    tokens: &[
        RouteTaskRisk::Low.as_label(),
        RouteTaskRisk::Medium.as_label(),
        RouteTaskRisk::High.as_label(),
        RouteTaskRisk::Critical.as_label(),
    ],
    descriptions: &[],
    ordered: true,
    self_report: false,
};

pub const CONFIDENCE_AXIS: RubricAxis = RubricAxis {
    name: "confidence",
    question: "your confidence in this assessment",
    tokens: &[
        RouteConfidence::Low.as_label(),
        RouteConfidence::Medium.as_label(),
        RouteConfidence::High.as_label(),
    ],
    descriptions: &[],
    ordered: true,
    self_report: true,
};

pub const INTENT_AXIS: RubricAxis = RubricAxis {
    name: zerocode_core::jev::questions::ROUTING_INTENT_ID,
    question: "what the task asks you to PRODUCE, in any language",
    tokens: &[
        RouteTaskIntent::Design.as_str(),
        RouteTaskIntent::Implementation.as_str(),
        RouteTaskIntent::Analysis.as_str(),
        RouteTaskIntent::Other.as_str(),
    ],
    descriptions: &[
        (
            RouteTaskIntent::Design.as_str(),
            "user-visible UI/UX or visual work \u{2014} a page, screen, component's look, layout, \
             theme, design system",
        ),
        (RouteTaskIntent::Implementation.as_str(), "non-visual code to write or change"),
        (RouteTaskIntent::Analysis.as_str(), "reading, explaining, investigating, reviewing"),
        (RouteTaskIntent::Other.as_str(), "anything else"),
    ],
    ordered: false,
    self_report: false,
};

/// The routing rubric — every axis, in the order the probe's reply lists them.
pub const ROUTING_RUBRIC: [RubricAxis; 4] =
    [COMPLEXITY_AXIS, RISK_AXIS, CONFIDENCE_AXIS, INTENT_AXIS];

/// The task text a routing judgment reads: description and prompt on their
/// own lines ([`rubric_task_whole`]), cut to [`RUBRIC_TASK_CHAR_CAP`]
/// characters on a character boundary, so a long CJK brief cannot split a
/// code point. The cut is the Jev door's own (`zerocode_core::jev::door::cut`),
/// so the chat probe and the typed judgment read the same head of a task.
#[must_use]
pub fn rubric_task_text(description: &str, prompt: &str) -> String {
    zerocode_core::jev::door::cut(
        &rubric_task_whole(description, prompt),
        zerocode_core::jev::Cap::Chars(RUBRIC_TASK_CHAR_CAP),
    )
}

/// The whole task a routing judgment is about, before any cut: description
/// and prompt on their own lines. The typed judgment hands this to the Jev
/// door, which withholds a line that may carry a credential before it cuts —
/// a cut first could leave half a secret the withholding no longer recognises.
#[must_use]
pub fn rubric_task_whole(description: &str, prompt: &str) -> String {
    format!("{description}\n{prompt}")
}

/// A parsed probe self-assessment. `confidence` is the probe's own stated
/// confidence in its verdict, not the router's signal-weight confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeAssessment {
    pub complexity: RouteTaskComplexity,
    pub risk: RouteTaskRisk,
    pub confidence: RouteConfidence,
    /// What the turn wants produced. `Other` whenever the probe did not say
    /// (an older probe model, a dropped field, an unknown token).
    pub intent: RouteTaskIntent,
}

impl ProbeAssessment {
    /// Every rubric axis the probe answered, with the token it answered in —
    /// the assessment as a ledger writes it, without naming its fields.
    #[must_use]
    pub fn tokens(&self) -> [(&'static RubricAxis, &'static str); 4] {
        [
            (&COMPLEXITY_AXIS, self.complexity.as_label()),
            (&RISK_AXIS, self.risk.as_label()),
            (&CONFIDENCE_AXIS, self.confidence.as_label()),
            (&INTENT_AXIS, self.intent.as_str()),
        ]
    }
}

/// Whether a fused route assessment is still table-only or carries a probe
/// verdict that cleared the confidence gate.
///
/// This is deliberately explicit provenance rather than something consumers
/// reconstruct from the fused values: an agreeing probe and a discarded probe
/// can produce identical values but have different authority to lower effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RouteAssessmentProvenance {
    #[default]
    Deterministic,
    TrustedProbe,
}

/// What fusion did with a probe, for audit notes and outcome attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFusionEffect {
    /// Probe agreed with (or was ignored in favor of) the deterministic verdict.
    Unchanged,
    /// Probe moved complexity up one band (or supplied it for `Unknown`).
    RaisedComplexity,
    /// Probe moved complexity down one band.
    LoweredComplexity,
}

/// Fused verdict plus the effect that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeFusion {
    pub complexity: RouteTaskComplexity,
    pub risk: RouteTaskRisk,
    /// The caller-supplied intent fallback, replaced by the probe's read only
    /// above the confidence gate. Turn routing supplies a neutral `Other`
    /// fallback so only a trusted probe can arm intent-driven behavior.
    pub intent: RouteTaskIntent,
    pub effect: ProbeFusionEffect,
    pub provenance: RouteAssessmentProvenance,
}

const fn complexity_rank(complexity: RouteTaskComplexity) -> Option<i8> {
    match complexity {
        RouteTaskComplexity::Trivial => Some(0),
        RouteTaskComplexity::Small => Some(1),
        RouteTaskComplexity::Medium => Some(2),
        RouteTaskComplexity::Large => Some(3),
        RouteTaskComplexity::Unknown => None,
    }
}

const fn complexity_from_rank(rank: i8) -> RouteTaskComplexity {
    match rank {
        i8::MIN..=0 => RouteTaskComplexity::Trivial,
        1 => RouteTaskComplexity::Small,
        2 => RouteTaskComplexity::Medium,
        _ => RouteTaskComplexity::Large,
    }
}

const fn risk_rank(risk: RouteTaskRisk) -> Option<i8> {
    match risk {
        RouteTaskRisk::Low => Some(0),
        RouteTaskRisk::Medium => Some(1),
        RouteTaskRisk::High => Some(2),
        RouteTaskRisk::Critical => Some(3),
        RouteTaskRisk::Unknown => None,
    }
}

/// Fuse a probe over the deterministic verdict.
///
/// Complexity rules, in order:
/// - probe `Low` confidence, or probe `Unknown` complexity: ignored.
/// - deterministic `Unknown`: probe supplies its value outright (there was
///   nothing to anchor to).
/// - probe `High` confidence: move one band toward the probe's value.
/// - probe `Medium` confidence: move one band toward the probe's value only
///   when that direction is *up* — an under-modeled task costs quality, an
///   over-modeled one only costs tokens, so the downgrade bar is higher.
///
/// Risk: a probe may only raise risk (any confidence above the gate); the
/// deterministic keyword tables stay the floor. Risk gates hard safety
/// behavior downstream (exploration gates, diversity), so it is never
/// lowered on a model's say-so.
///
/// Intent: the caller-supplied value is the fallback. A probe that clears the
/// confidence gate replaces it verbatim, including with `Other`; below the
/// gate the fallback survives unchanged. Turn routing always supplies `Other`.
#[must_use]
pub fn fuse_probe_assessment(
    deterministic_complexity: RouteTaskComplexity,
    deterministic_risk: RouteTaskRisk,
    deterministic_intent: RouteTaskIntent,
    probe: &ProbeAssessment,
) -> ProbeFusion {
    let mut fusion = ProbeFusion {
        complexity: deterministic_complexity,
        risk: deterministic_risk,
        intent: deterministic_intent,
        effect: ProbeFusionEffect::Unchanged,
        provenance: RouteAssessmentProvenance::Deterministic,
    };
    if !matches!(probe.confidence, RouteConfidence::Low) {
        fusion.provenance = RouteAssessmentProvenance::TrustedProbe;
        fusion.intent = probe.intent;
        if let Some(probe_risk) = risk_rank(probe.risk) {
            if risk_rank(deterministic_risk).is_none_or(|current| probe_risk > current) {
                fusion.risk = probe.risk;
            }
        }
        fusion = fuse_complexity(deterministic_complexity, *probe, fusion);
    }
    fusion
}

fn fuse_complexity(
    deterministic: RouteTaskComplexity,
    probe: ProbeAssessment,
    mut fusion: ProbeFusion,
) -> ProbeFusion {
    let Some(probe_rank) = complexity_rank(probe.complexity) else {
        return fusion;
    };
    let Some(current) = complexity_rank(deterministic) else {
        fusion.complexity = probe.complexity;
        fusion.effect = ProbeFusionEffect::RaisedComplexity;
        return fusion;
    };
    let step = (probe_rank - current).signum();
    let allowed = match probe.confidence {
        RouteConfidence::High => step != 0,
        RouteConfidence::Medium => step > 0,
        RouteConfidence::Low => false,
    };
    if allowed {
        fusion.complexity = complexity_from_rank(current + step);
        fusion.effect = if step > 0 {
            ProbeFusionEffect::RaisedComplexity
        } else {
            ProbeFusionEffect::LoweredComplexity
        };
    }
    fusion
}

/// Who the probe is told it is, and the one reply shape it may give; the
/// rubric's JSON contract follows on the next line.
const PROBE_PREAMBLE: &str = "You are a routing classifier. Assess the software task below and \
                              reply with ONLY this JSON object, no prose:\n";
/// Where the task text begins, after the rubric.
const PROBE_TASK_HEADING: &str = "\n\nTASK:\n";

/// The probe prompt up to its task heading — rendered from [`ROUTING_RUBRIC`]
/// once per process, since nothing in it varies by task.
fn probe_prompt_head() -> &'static str {
    static HEAD: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HEAD.get_or_init(|| {
        use std::fmt::Write as _;
        let mut head = String::from(PROBE_PREAMBLE);
        head.push('{');
        for (index, axis) in ROUTING_RUBRIC.iter().enumerate() {
            if index > 0 {
                head.push(',');
            }
            let _ = write!(head, "\"{}\":\"{}\"", axis.name, axis.tokens.join("|"));
        }
        head.push_str("}\n");
        for (index, axis) in ROUTING_RUBRIC.iter().enumerate() {
            if index > 0 {
                head.push(' ');
            }
            let _ = write!(head, "{} = {}", axis.name, axis.question);
            for (position, (token, description)) in axis.descriptions.iter().enumerate() {
                head.push_str(if position == 0 { " (" } else { "; " });
                let _ = write!(head, "{token}: {description}");
            }
            if !axis.descriptions.is_empty() {
                head.push(')');
            }
            head.push('.');
        }
        head.push_str(PROBE_TASK_HEADING);
        head
    })
}

/// The probe's instruction text, rendered from [`ROUTING_RUBRIC`]. Kept terse
/// on purpose: the probe runs on a Fast-tier model with a hard output budget,
/// and the JSON contract is the entire interface — [`parse_probe_response`]
/// rejects anything else.
#[must_use]
pub fn probe_prompt(description: &str, prompt: &str) -> String {
    let head = probe_prompt_head();
    let task = rubric_task_text(description, prompt);
    let mut rendered = String::with_capacity(head.len() + task.len());
    rendered.push_str(head);
    rendered.push_str(&task);
    rendered
}

/// Parse the probe's reply. Strict by design on the THREE routing fields: the
/// first `{{…}}` JSON object found is parsed, and a complexity/risk/confidence
/// token its rubric axis does not offer rejects the whole probe (fail closed
/// to the deterministic verdict rather than guess what a malformed probe
/// meant).
///
/// `intent` is the deliberate exception and is parsed LENIENTLY — absent,
/// null, or an unrecognized token all yield [`RouteTaskIntent::Other`] while
/// the rest of the probe still parses. The axis is additive: a probe model
/// that has never heard of it must keep working exactly as it did before,
/// because a probe that stops parsing is a probe that stops routing.
#[must_use]
pub fn parse_probe_response(raw: &str) -> Option<ProbeAssessment> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&raw[start..=end]).ok()?;
    // The axis's offer is checked before the enum is asked, so a label the
    // enum knows but the rubric does not offer (`unknown`) still rejects.
    let offered = |axis: &RubricAxis| {
        value
            .get(axis.name)
            .and_then(serde_json::Value::as_str)
            .map(|token| token.trim().to_ascii_lowercase())
            .filter(|token| axis.position(token).is_some())
    };
    let complexity = RouteTaskComplexity::from_label(&offered(&COMPLEXITY_AXIS)?)?;
    let risk = RouteTaskRisk::from_label(&offered(&RISK_AXIS)?)?;
    let confidence = RouteConfidence::from_label(&offered(&CONFIDENCE_AXIS)?)?;
    let intent = offered(&INTENT_AXIS)
        .and_then(|token| RouteTaskIntent::from_label(&token))
        .unwrap_or_default();
    Some(ProbeAssessment { complexity, risk, confidence, intent })
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn probe(
        complexity: RouteTaskComplexity,
        risk: RouteTaskRisk,
        confidence: RouteConfidence,
    ) -> ProbeAssessment {
        ProbeAssessment { complexity, risk, confidence, intent: RouteTaskIntent::Other }
    }

    const fn probe_with_intent(
        complexity: RouteTaskComplexity,
        confidence: RouteConfidence,
        intent: RouteTaskIntent,
    ) -> ProbeAssessment {
        ProbeAssessment { complexity, risk: RouteTaskRisk::Low, confidence, intent }
    }

    #[test]
    fn high_confidence_probe_moves_one_band_not_more() {
        let fusion = fuse_probe_assessment(
            RouteTaskComplexity::Small,
            RouteTaskRisk::Low,
            RouteTaskIntent::Other,
            &probe(RouteTaskComplexity::Large, RouteTaskRisk::Low, RouteConfidence::High),
        );
        assert_eq!(fusion.complexity, RouteTaskComplexity::Medium);
        assert_eq!(fusion.effect, ProbeFusionEffect::RaisedComplexity);
    }

    #[test]
    fn medium_confidence_upgrades_but_never_downgrades() {
        let up = fuse_probe_assessment(
            RouteTaskComplexity::Small,
            RouteTaskRisk::Low,
            RouteTaskIntent::Other,
            &probe(RouteTaskComplexity::Medium, RouteTaskRisk::Low, RouteConfidence::Medium),
        );
        assert_eq!(up.complexity, RouteTaskComplexity::Medium);
        let down = fuse_probe_assessment(
            RouteTaskComplexity::Large,
            RouteTaskRisk::Low,
            RouteTaskIntent::Other,
            &probe(RouteTaskComplexity::Small, RouteTaskRisk::Low, RouteConfidence::Medium),
        );
        assert_eq!(down.complexity, RouteTaskComplexity::Large);
        assert_eq!(down.effect, ProbeFusionEffect::Unchanged);
    }

    #[test]
    fn low_confidence_probe_is_ignored_entirely() {
        let fusion = fuse_probe_assessment(
            RouteTaskComplexity::Small,
            RouteTaskRisk::Low,
            RouteTaskIntent::Design,
            &probe(RouteTaskComplexity::Large, RouteTaskRisk::Critical, RouteConfidence::Low),
        );
        assert_eq!(fusion.complexity, RouteTaskComplexity::Small);
        assert_eq!(fusion.risk, RouteTaskRisk::Low);
        assert_eq!(fusion.intent, RouteTaskIntent::Design);
        assert_eq!(fusion.effect, ProbeFusionEffect::Unchanged);
        assert_eq!(
            fusion.provenance,
            RouteAssessmentProvenance::Deterministic
        );
    }

    #[test]
    fn unknown_deterministic_complexity_takes_probe_value() {
        let fusion = fuse_probe_assessment(
            RouteTaskComplexity::Unknown,
            RouteTaskRisk::Unknown,
            RouteTaskIntent::Other,
            &probe(RouteTaskComplexity::Large, RouteTaskRisk::Medium, RouteConfidence::High),
        );
        assert_eq!(fusion.complexity, RouteTaskComplexity::Large);
        assert_eq!(fusion.risk, RouteTaskRisk::Medium);
        assert_eq!(fusion.effect, ProbeFusionEffect::RaisedComplexity);
    }

    #[test]
    fn risk_only_ever_rises() {
        let fusion = fuse_probe_assessment(
            RouteTaskComplexity::Medium,
            RouteTaskRisk::High,
            RouteTaskIntent::Other,
            &probe(RouteTaskComplexity::Medium, RouteTaskRisk::Low, RouteConfidence::High),
        );
        assert_eq!(fusion.risk, RouteTaskRisk::High);
        let raised = fuse_probe_assessment(
            RouteTaskComplexity::Medium,
            RouteTaskRisk::Low,
            RouteTaskIntent::Other,
            &probe(RouteTaskComplexity::Medium, RouteTaskRisk::High, RouteConfidence::Medium),
        );
        assert_eq!(raised.risk, RouteTaskRisk::High);
    }

    #[test]
    fn parse_accepts_fenced_json_and_rejects_unknown_tokens() {
        let parsed = parse_probe_response(
            "```json\n{\"complexity\":\"large\",\"risk\":\"medium\",\"confidence\":\"high\"}\n```",
        )
        .expect("fenced JSON parses");
        assert_eq!(parsed.complexity, RouteTaskComplexity::Large);
        assert_eq!(parsed.risk, RouteTaskRisk::Medium);
        assert_eq!(parsed.confidence, RouteConfidence::High);
        assert!(parse_probe_response(
            "{\"complexity\":\"gigantic\",\"risk\":\"medium\",\"confidence\":\"high\"}"
        )
        .is_none());
        assert!(parse_probe_response("no json here").is_none());
    }

    #[test]
    fn parse_reads_the_intent_axis_and_defaults_it_when_absent() {
        let with_intent = parse_probe_response(
            "{\"complexity\":\"small\",\"risk\":\"low\",\"confidence\":\"high\",\"intent\":\"DESIGN\"}",
        )
        .expect("intent-bearing JSON parses");
        assert_eq!(with_intent.intent, RouteTaskIntent::Design);

        // The pre-intent contract: three fields, still a full parse.
        let legacy =
            parse_probe_response("{\"complexity\":\"small\",\"risk\":\"low\",\"confidence\":\"high\"}")
                .expect("a probe without the intent field must still parse");
        assert_eq!(legacy.intent, RouteTaskIntent::Other);
        assert_eq!(legacy.complexity, RouteTaskComplexity::Small);
    }

    #[test]
    fn unknown_intent_tokens_fail_open_without_rejecting_the_probe() {
        for raw in [
            "{\"complexity\":\"medium\",\"risk\":\"low\",\"confidence\":\"high\",\"intent\":\"vibes\"}",
            "{\"complexity\":\"medium\",\"risk\":\"low\",\"confidence\":\"high\",\"intent\":null}",
            "{\"complexity\":\"medium\",\"risk\":\"low\",\"confidence\":\"high\",\"intent\":7}",
        ] {
            let parsed = parse_probe_response(raw)
                .unwrap_or_else(|| panic!("a bad intent must not reject the probe: {raw}"));
            assert_eq!(parsed.intent, RouteTaskIntent::Other);
            assert_eq!(parsed.complexity, RouteTaskComplexity::Medium);
        }
    }

    #[test]
    fn fusion_replaces_seeded_intent_only_above_the_confidence_gate() {
        for confidence in [RouteConfidence::Medium, RouteConfidence::High] {
            let fusion = fuse_probe_assessment(
                RouteTaskComplexity::Small,
                RouteTaskRisk::Low,
                RouteTaskIntent::Implementation,
                &probe_with_intent(RouteTaskComplexity::Small, confidence, RouteTaskIntent::Design),
            );
            assert_eq!(fusion.intent, RouteTaskIntent::Design, "{confidence:?}");
            assert_eq!(
                fusion.provenance,
                RouteAssessmentProvenance::TrustedProbe,
                "{confidence:?}"
            );
        }
        let discarded = fuse_probe_assessment(
            RouteTaskComplexity::Small,
            RouteTaskRisk::Low,
            RouteTaskIntent::Implementation,
            &probe_with_intent(
                RouteTaskComplexity::Small,
                RouteConfidence::Low,
                RouteTaskIntent::Design,
            ),
        );
        assert_eq!(
            discarded.intent,
            RouteTaskIntent::Implementation,
            "a low-confidence probe must preserve the deterministic fallback"
        );
    }

    #[test]
    fn probe_prompt_requests_the_intent_field() {
        let prompt = probe_prompt("", "랜딩페이지 만들어줘");
        assert!(prompt.contains("\"intent\":\"design|implementation|analysis|other\""));
        assert!(prompt.contains("in any language"));
    }

    /// The probe prompt, byte for byte, as the probe has been answering it.
    /// Every render change must keep this green: the routing probe's behavior
    /// is pinned by the words it is shown, and a table that re-spells them
    /// has changed the probe.
    const PROBE_PROMPT_GOLDEN_HEAD: &str = "You are a routing classifier. Assess the software task below and reply with ONLY this JSON object, no prose:\n{\"complexity\":\"trivial|small|medium|large\",\"risk\":\"low|medium|high|critical\",\"confidence\":\"low|medium|high\",\"intent\":\"design|implementation|analysis|other\"}\ncomplexity = how much reasoning/context the task needs end to end (large: repo-wide, multi-subsystem, or architecturally hard; trivial: a label/typo-class edit). risk = blast radius of a wrong edit (credentials, deletion, security = high+). confidence = your confidence in this assessment. intent = what the task asks you to PRODUCE, in any language (design: user-visible UI/UX or visual work \u{2014} a page, screen, component's look, layout, theme, design system; implementation: non-visual code to write or change; analysis: reading, explaining, investigating, reviewing; other: anything else).\n\nTASK:\n";

    #[test]
    fn the_probe_prompt_is_byte_identical_to_the_golden() {
        assert_eq!(
            probe_prompt("설명", "랜딩페이지 만들어줘"),
            format!("{PROBE_PROMPT_GOLDEN_HEAD}설명\n랜딩페이지 만들어줘")
        );
        // A turn probe has no description: the task still starts on its own line.
        assert_eq!(
            probe_prompt("", "fix the typo in README"),
            format!("{PROBE_PROMPT_GOLDEN_HEAD}\nfix the typo in README")
        );
        // Past the cap the task keeps its first 2,000 characters, cut on a
        // character boundary.
        let long = "레포".repeat(3_000);
        let capped: String = format!("설명\n{long}").chars().take(2_000).collect();
        assert_eq!(probe_prompt("설명", &long), format!("{PROBE_PROMPT_GOLDEN_HEAD}{capped}"));
        let exact: String = "x".repeat(2_000 - 1);
        assert_eq!(probe_prompt("", &exact), format!("{PROBE_PROMPT_GOLDEN_HEAD}\n{exact}"));
    }

    /// The rubric speaks the router's own vocabulary: every token an axis
    /// offers reads back into its enum, and an ordered axis lists its tokens
    /// in the same order fusion ranks them — the order an under-estimate is
    /// measured against.
    #[test]
    fn the_rubric_offers_the_router_enums_own_labels_in_rank_order() {
        let complexity: Vec<RouteTaskComplexity> = COMPLEXITY_AXIS
            .tokens
            .iter()
            .map(|token| RouteTaskComplexity::from_label(token).expect("a complexity label"))
            .collect();
        assert!(complexity.windows(2).all(|pair| complexity_rank(pair[0]) < complexity_rank(pair[1])));
        let risk: Vec<RouteTaskRisk> = RISK_AXIS
            .tokens
            .iter()
            .map(|token| RouteTaskRisk::from_label(token).expect("a risk label"))
            .collect();
        assert!(risk.windows(2).all(|pair| risk_rank(pair[0]) < risk_rank(pair[1])));
        for token in CONFIDENCE_AXIS.tokens {
            assert!(RouteConfidence::from_label(token).is_some(), "{token}");
        }
        assert_eq!(
            INTENT_AXIS.tokens.len(),
            RouteTaskIntent::ALL.len(),
            "every intent is offered"
        );
        for token in INTENT_AXIS.tokens {
            assert!(RouteTaskIntent::from_label(token).is_some(), "{token}");
        }
        // Nothing the enums use for "not known" is ever offered as an answer.
        for axis in ROUTING_RUBRIC {
            assert!(axis.position(RouteTaskComplexity::Unknown.as_label()).is_none(), "{}", axis.name);
            for (token, _) in axis.descriptions {
                assert!(axis.position(token).is_some(), "{} describes {token}", axis.name);
            }
        }
        // Exactly one axis is the reader's report on itself.
        let self_reports: Vec<&str> =
            ROUTING_RUBRIC.iter().filter(|axis| axis.self_report).map(|axis| axis.name).collect();
        assert_eq!(self_reports, vec![CONFIDENCE_AXIS.name]);
        // An assessment names every axis once, in rubric order, with a token
        // the axis offers.
        let assessment = probe(RouteTaskComplexity::Large, RouteTaskRisk::High, RouteConfidence::Medium);
        let tokens = assessment.tokens();
        assert_eq!(tokens.map(|(axis, _)| axis.name), ROUTING_RUBRIC.map(|axis| axis.name));
        for (axis, token) in tokens {
            assert!(axis.position(token).is_some(), "{}:{token}", axis.name);
        }
    }

    #[test]
    fn the_task_text_is_one_function_for_every_reader() {
        let long = "레포".repeat(3_000);
        let task = rubric_task_text("설명", &long);
        assert_eq!(task.chars().count(), RUBRIC_TASK_CHAR_CAP);
        assert!(probe_prompt("설명", &long).ends_with(&task));
        assert_eq!(rubric_task_text("", "short"), "\nshort");
        let at_cap = "a".repeat(RUBRIC_TASK_CHAR_CAP - 1);
        assert_eq!(rubric_task_text("", &at_cap).chars().count(), RUBRIC_TASK_CHAR_CAP);
    }

    /// Route-outcome records spelled complexity, risk and the probe's
    /// confidence from each variant's `Debug` name until every enum had one
    /// label function the rubric reads too; records keep the same bytes.
    #[test]
    fn the_label_functions_spell_what_route_records_always_kept() {
        let debug = |value: &dyn std::fmt::Debug| format!("{value:?}").to_ascii_lowercase();
        for complexity in RouteTaskComplexity::ALL {
            assert_eq!(complexity.as_label(), debug(&complexity));
        }
        for risk in RouteTaskRisk::ALL {
            assert_eq!(risk.as_label(), debug(&risk));
        }
        for confidence in RouteConfidence::ALL {
            assert_eq!(confidence.as_label(), debug(&confidence));
        }
    }

    #[test]
    fn a_label_the_enum_knows_but_the_rubric_does_not_offer_rejects_the_probe() {
        assert!(parse_probe_response(
            "{\"complexity\":\"unknown\",\"risk\":\"low\",\"confidence\":\"high\"}"
        )
        .is_none());
        assert!(parse_probe_response(
            "{\"complexity\":\"small\",\"risk\":\"unknown\",\"confidence\":\"high\"}"
        )
        .is_none());
        let spaced = parse_probe_response(
            "{\"complexity\":\" LARGE \",\"risk\":\"Critical\",\"confidence\":\"LOW\",\"intent\":\" analysis\"}",
        )
        .expect("case and padding are the probe's, not the rubric's");
        assert_eq!(spaced.complexity, RouteTaskComplexity::Large);
        assert_eq!(spaced.risk, RouteTaskRisk::Critical);
        assert_eq!(spaced.confidence, RouteConfidence::Low);
        assert_eq!(spaced.intent, RouteTaskIntent::Analysis);
        assert!(parse_probe_response("{\"complexity\":7,\"risk\":\"low\",\"confidence\":\"high\"}").is_none());
    }

    #[test]
    fn probe_prompt_caps_input_on_char_boundary() {
        let long_task = "레포".repeat(3_000);
        let prompt = probe_prompt("설명", &long_task);
        // Only the TASK is capped (2_000 chars); the rest is the fixed
        // instruction block, so bound the total by cap + instruction size.
        let instruction_chars = probe_prompt("", "").chars().count();
        assert!(prompt.chars().count() <= 2_000 + instruction_chars);
        assert!(prompt.contains("routing classifier"));
    }
}
