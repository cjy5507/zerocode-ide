//! Pre-execution goal-ambiguity screen: catch a goal whose success metric can
//! be read two ways BEFORE hours are spent on the wrong reading.
//!
//! The observed 41-hour runaway began with "make it 100% coverage": the agent
//! optimized Go *statement* coverage to exactly 100.0% for twelve hours while
//! the user meant *requirement* coverage of a ticket catalog. No guard can
//! recover that loss after the fact — the only cheap fix is one clarifying
//! question before work starts.
//!
//! [`screen_goal`] is deliberately conservative (a false positive here nags a
//! user who was perfectly clear — firing must be rare and obviously right).
//! It reports [`GoalAmbiguity::Ambiguous`] only when ALL THREE hold:
//!
//! 1. a **totality quantifier** is present ("100%", "완벽", "전부", "perfect",
//!    "fully", …) — the goal demands an extreme of something, and
//! 2. an **ambiguous-metric noun** is present ("커버리지"/"coverage",
//!    "최적화"/"optimize", "성능"/"performance", …) — the something has more
//!    than one standard reading, and
//! 3. **no decidable criterion** appears in the text (no check command like
//!    `cargo`/`grep:`/`pytest`, no `--check` flag) — nothing in the goal pins
//!    which reading was meant.
//!
//! Each cue carries its standard interpretations so the caller can ask ONE
//! precise question with concrete options instead of a vague "무슨 뜻인가요?".
//! Deterministic and total: no model call, no IO, korean/english parity.

/// One ambiguous metric found in a goal, with its standard readings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguityCue {
    /// The metric noun that fired (as listed in the table, not as matched).
    pub term: &'static str,
    /// The distinct standard readings the user should pick between.
    pub interpretations: &'static [&'static str],
}

/// The screen's verdict for one goal text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalAmbiguity {
    /// Unambiguous as scoped (or already carries a decidable criterion) —
    /// start immediately, never nag.
    Clear,
    /// A totality quantifier meets an ambiguous metric with nothing pinning
    /// the reading: ask one clarifying question before starting.
    Ambiguous(Vec<AmbiguityCue>),
}

/// Totality quantifiers: the goal demands an extreme (KR/EN parity). Matched
/// case-insensitively against the whole text.
const QUANTIFIERS: &[&str] = &[
    "100%",
    "100 %",
    "100프로",
    "100퍼센트",
    "100 percent",
    "완벽",
    "전부",
    "모두",
    "모든",
    "빠짐없이",
    "perfect",
    "flawless",
    "fully",
    "entirely",
    "every ",
    "exhaustive",
];

/// Ambiguous metric nouns and their standard readings. `terms` are the match
/// patterns; the cue reports the first term as its display name.
const METRICS: &[(&[&str], &[&str])] = &[
    (
        &["커버리지", "커버리", "coverage"],
        &[
            "테스트 커버리지 % (문장/분기)",
            "요구사항(티켓/카탈로그) 충족 커버리지",
        ],
    ),
    (
        &["최적화", "optimiz"],
        &[
            "실행 속도 (어떤 벤치마크 기준?)",
            "메모리 사용량",
            "빌드 시간",
            "코드 크기/가독성",
        ],
    ),
    (
        &["성능", "performance"],
        &["지연시간 (p50? p99?)", "처리량 (req/s)", "리소스 사용량"],
    ),
    (
        &["품질", "quality"],
        &[
            "테스트 전부 그린",
            "린트/정적분석 클린",
            "리뷰 지적 0건",
        ],
    ),
    (
        &["정리", "clean up", "cleanup"],
        &["죽은 코드 제거", "포맷/린트 정리", "구조 리팩터링"],
    ),
];

/// Decidable-criterion markers: any of these in the goal text pins the metric
/// (a named check command, an explicit validator, or the `--check` flag), so
/// the screen must stay quiet no matter what else the text says. `make`/`just`
/// are deliberately absent — as English words they appear in ordinary goal
/// prose ("make coverage perfect") and would silence real ambiguity.
const DECIDABLE: &[&str] = &[
    "cargo",
    "npm ",
    "pnpm",
    "yarn ",
    "pytest",
    "go test",
    "grep:",
    "git:",
    "--check",
    "--until",
    // An explicit criterion phrase pins the metric in natural Korean prose
    // ("최적화 기준: 메모리 사용량"). This is also the marker the TUI's
    // clarify picker appends when the user answers the one-question hold,
    // so an answered goal can never re-trigger the gate into a modal loop.
    "기준:",
    "criterion:",
];

/// Rewrite a held goal with the user's chosen reading pinned as an explicit
/// criterion. This is the ONLY place the pin phrase is spelled: it embeds a
/// `DECIDABLE` marker (`기준:`), so the returned text can never re-trigger
/// [`screen_goal`] — the clarify picker's answer path and the screen share
/// one source of truth instead of two surfaces that could drift (the
/// round-trip property is pinned by `a_pinned_goal_never_refires`).
#[must_use]
pub fn pin_goal_criterion(goal: &str, term: &str, reading: &str) -> String {
    format!("{goal} — {term} 기준: {reading}")
}

/// Screen one goal text. See the module docs for the three-way AND rule.
#[must_use]
pub fn screen_goal(text: &str) -> GoalAmbiguity {
    let haystack = text.to_lowercase();
    if haystack.trim().is_empty() {
        return GoalAmbiguity::Clear;
    }
    if DECIDABLE.iter().any(|marker| haystack.contains(marker)) {
        return GoalAmbiguity::Clear;
    }
    if !QUANTIFIERS.iter().any(|q| haystack.contains(q)) {
        return GoalAmbiguity::Clear;
    }
    let cues: Vec<AmbiguityCue> = METRICS
        .iter()
        .filter(|(terms, _)| terms.iter().any(|term| haystack.contains(term)))
        .map(|(terms, interpretations)| AmbiguityCue {
            term: terms[0],
            interpretations,
        })
        .collect();
    if cues.is_empty() {
        GoalAmbiguity::Clear
    } else {
        GoalAmbiguity::Ambiguous(cues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_ambiguous(text: &str) -> bool {
        matches!(screen_goal(text), GoalAmbiguity::Ambiguous(_))
    }

    #[test]
    fn the_runaway_goal_fires_with_the_coverage_cue() {
        // The literal 41h-runaway opener.
        match screen_goal("100프로 커버리지 만들어") {
            GoalAmbiguity::Ambiguous(cues) => {
                assert_eq!(cues.len(), 1);
                assert_eq!(cues[0].term, "커버리지");
                assert!(
                    cues[0].interpretations.len() >= 2,
                    "a cue must offer the distinct readings to pick between"
                );
            }
            GoalAmbiguity::Clear => panic!("the canonical runaway goal must fire"),
        }
    }

    #[test]
    fn english_and_korean_parity() {
        assert!(is_ambiguous("make coverage perfect"));
        assert!(is_ambiguous("완벽하게 최적화해줘"));
        assert!(is_ambiguous("fully optimize the pipeline"));
        assert!(is_ambiguous("성능을 완벽하게 튜닝해"));
    }

    #[test]
    fn a_decidable_criterion_always_silences_the_screen() {
        // Same ambiguous wording, but a named check pins the reading.
        assert!(!is_ambiguous("100프로 커버리지 만들어, 검증은 cargo:test"));
        assert!(!is_ambiguous("make coverage perfect --check \"go test -cover\""));
        assert!(!is_ambiguous("완벽하게 최적화해줘 grep:BENCH_OK"));
        assert!(!is_ambiguous("전부 커버리지 --until grep:DONE"));
    }

    /// `기준:` 은 clarify 픽커가 답을 각인하는 마커: 답한 goal이 게이트를
    /// 다시 발화시키면 모달 루프가 되므로, 이 형태는 반드시 침묵해야 한다.
    #[test]
    fn an_explicit_criterion_phrase_silences_the_screen() {
        assert!(!is_ambiguous("완벽하게 최적화해줘 — 최적화 기준: 메모리 사용량"));
        assert!(!is_ambiguous("fully optimize the pipeline — criterion: p99 latency"));
    }

    /// 라운드트립 속성: 발화한 어떤 goal이든, 그 cue의 어떤 해석으로
    /// [`pin_goal_criterion`]을 통과시키면 다시는 발화하지 않는다 — 픽커
    /// 문구와 스크린 마커가 한 소스라는 계약 자체를 핀한다.
    #[test]
    fn a_pinned_goal_never_refires() {
        for goal in ["완벽하게 최적화해줘", "make coverage perfect", "성능을 완벽하게 튜닝해"] {
            let GoalAmbiguity::Ambiguous(cues) = screen_goal(goal) else {
                panic!("fixture goal must fire: {goal}");
            };
            for cue in &cues {
                for reading in cue.interpretations {
                    let pinned = pin_goal_criterion(goal, cue.term, reading);
                    assert_eq!(
                        screen_goal(&pinned),
                        GoalAmbiguity::Clear,
                        "pinned goal must never re-trigger the screen: {pinned}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_quantifier_without_an_ambiguous_metric_is_clear() {
        assert!(!is_ambiguous("모든 파일 포맷 맞춰줘"));
        assert!(!is_ambiguous("fully document the public API"));
        assert!(!is_ambiguous("전부 커밋해줘"));
    }

    #[test]
    fn an_ambiguous_metric_without_a_quantifier_is_clear() {
        // Asking ABOUT coverage/optimization is not demanding an extreme of it.
        assert!(!is_ambiguous("커버리지 리포트 보여줘"));
        assert!(!is_ambiguous("이 쿼리 최적화 여지 분석해줘"));
        assert!(!is_ambiguous("check the performance of the worker"));
    }

    #[test]
    fn ordinary_goals_never_fire() {
        for text in [
            "이 함수 오타 고쳐",
            "fix the login bug",
            "카탈로그 16~28 요구사항 반영하고 dev에 배포",
            "add a unit test for the parser",
            "",
            "   ",
        ] {
            assert!(!is_ambiguous(text), "{text:?} must be Clear");
        }
    }

    #[test]
    fn multiple_cues_all_reported() {
        match screen_goal("성능이랑 커버리지 전부 완벽하게") {
            GoalAmbiguity::Ambiguous(cues) => {
                let terms: Vec<_> = cues.iter().map(|cue| cue.term).collect();
                assert!(terms.contains(&"커버리지"));
                assert!(terms.contains(&"성능"));
            }
            GoalAmbiguity::Clear => panic!("both cues must fire"),
        }
    }
}
