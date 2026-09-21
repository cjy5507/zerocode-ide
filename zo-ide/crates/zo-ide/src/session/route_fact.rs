//! The route a turn is on, as the seats that chose it already know it
//! (t-5872): what the status row says beside `Working`.
//!
//! Nothing here asks Jev anything. The two places that already hold a
//! decision hand it over: the turn's start, where the Smart install knows
//! the routing judgment's band and the effort floor it set; and the step
//! effort governor's observer, which the runtime tells every step's decision
//! — the model and effort the next request carries and the one-word reason.
//! Both write the same [`RouteFact`] into a `watch` channel the interactive
//! front reads between frames, so a person sees `jev · opus max ·
//! stuck_check_red` the moment the governor moves the wire, on the same row
//! codex keeps its inline context.

use tokio::sync::watch;

/// Who decided the route the fact describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteJudge {
    /// Jev's typed judgment — the routing seat at turn start, or the step
    /// seat's fresh band inside the turn.
    Jev,
    /// The Fast-tier chat probe cleared fusion's gate at turn start.
    Probe,
    /// The step table's own reading of the signals moved the wire.
    Step,
}

impl RouteJudge {
    /// The status row's word for the seat.
    #[must_use]
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::Probe => "probe",
            Self::Step => "step",
        }
    }
}

/// One decided route: the seat, the wire model when it is not the session's,
/// the effort the request carries and the one-word reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteFact {
    pub(crate) who: RouteJudge,
    pub(crate) model: Option<String>,
    pub(crate) effort: &'static str,
    pub(crate) reason: &'static str,
}

impl RouteFact {
    /// The turn's opening fact: the judged band and the effort floor the
    /// requests start on. `None` when nobody judged — the deterministic
    /// table's verdict is not a decision worth a word — or when the turn
    /// carries no effort to speak of.
    #[must_use]
    pub(crate) fn turn_start(
        assessment: tools::TurnProbeAssessment,
        effort: Option<api::EffortLevel>,
    ) -> Option<Self> {
        if assessment.provenance != runtime::RouteAssessmentProvenance::TrustedProbe {
            return None;
        }
        Some(Self {
            who: if assessment.judged { RouteJudge::Jev } else { RouteJudge::Probe },
            model: None,
            effort: effort?.label(),
            reason: assessment.complexity.as_label(),
        })
    }

    /// A step decision the observer was told: only one the next request
    /// carries (`applied`) and that moved something — a rung of effort, a
    /// model — or that a fresh judgment stood behind. A `working` row that
    /// changed nothing keeps the row as it was.
    #[must_use]
    pub(crate) fn step(row: &runtime::StepRow) -> Option<Self> {
        if !row.applied || (row.delta == 0 && row.jev.is_none() && row.rung_move.is_none()) {
            return None;
        }
        Some(Self {
            who: if row.jev.is_some() { RouteJudge::Jev } else { RouteJudge::Step },
            model: row.model.clone(),
            effort: row.effort_after,
            reason: row.reason,
        })
    }
}

/// The turn's side of the channel.
pub(crate) type RouteFactSender = watch::Sender<Option<RouteFact>>;
/// The front's side.
pub(crate) type RouteFactReceiver = watch::Receiver<Option<RouteFact>>;

/// A fresh channel with nothing decided yet.
#[must_use]
pub(crate) fn channel() -> RouteFactSender {
    watch::channel(None).0
}

#[cfg(test)]
mod tests {
    use super::{RouteFact, RouteJudge};

    fn assessment(provenance: runtime::RouteAssessmentProvenance, judged: bool) -> tools::TurnProbeAssessment {
        tools::TurnProbeAssessment {
            complexity: runtime::RouteTaskComplexity::Large,
            intent: runtime::RouteTaskIntent::Other,
            provenance,
            judged,
        }
    }

    #[test]
    fn a_turn_start_fact_needs_a_judged_band_and_an_effort() {
        use runtime::RouteAssessmentProvenance as P;
        assert_eq!(RouteFact::turn_start(assessment(P::Deterministic, true), Some(api::EffortLevel::Xhigh)), None);
        assert_eq!(RouteFact::turn_start(assessment(P::TrustedProbe, true), None), None);
        assert_eq!(
            RouteFact::turn_start(assessment(P::TrustedProbe, true), Some(api::EffortLevel::Xhigh)),
            Some(RouteFact { who: RouteJudge::Jev, model: None, effort: "xhigh", reason: "large" })
        );
        assert_eq!(
            RouteFact::turn_start(assessment(P::TrustedProbe, false), Some(api::EffortLevel::High)).map(|fact| fact.who),
            Some(RouteJudge::Probe)
        );
    }

    fn row(applied: bool, delta: i8, jev: bool) -> runtime::StepRow {
        runtime::StepRow {
            kind: runtime::STEP_ROW_KIND,
            at: 0,
            attempt: "turn-1@1".to_string(),
            step: 3,
            model: Some("claude-opus-5".to_string()),
            band: "large",
            batch: "check",
            repeats: 0,
            error_streak: 0,
            check_red: true,
            routine_streak: 0,
            delta,
            effort_before: "xhigh",
            effort_after: "max",
            ceiling_before: None,
            ceiling_after: None,
            reason: "stuck_check_red",
            applied,
            held: None,
            ask: None,
            jev: jev.then_some(runtime::StepJudgmentRow { complexity: "large", delta: Some(1), at_step: 2 }),
            rung_move: None,
        }
    }

    #[test]
    fn a_step_fact_is_only_an_applied_move_and_names_its_seat() {
        assert_eq!(RouteFact::step(&row(false, 2, false)), None, "a shadow row moves no wire");
        assert_eq!(RouteFact::step(&row(true, 0, false)), None, "an unchanged step says nothing new");
        let table = RouteFact::step(&row(true, 2, false)).expect("an applied move");
        assert_eq!(
            table,
            RouteFact {
                who: RouteJudge::Step,
                model: Some("claude-opus-5".to_string()),
                effort: "max",
                reason: "stuck_check_red",
            }
        );
        assert_eq!(RouteFact::step(&row(true, 2, true)).map(|fact| fact.who), Some(RouteJudge::Jev));
        assert_eq!(RouteJudge::Jev.word(), "jev");
    }
}
