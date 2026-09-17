//! The one word table that says which orchestration shape a person asked for.
//!
//! Two different questions read it and keep their own meanings: [`evidence`]
//! asks "which shape did the person REQUEST" (the answer becomes
//! `user_requested_delegation`, the host's explicit-delegation escape), while
//! [`metadata`] asks "does this non-coding task read as lane work" (the answer
//! is `Large` complexity). What they must not keep is their own COPY of the
//! words: a second table is exactly how 「병렬」 came to be a Large-complexity
//! marker in `metadata` while a Korean 「병렬로 분석해」 never set
//! `user_requested_delegation` in `evidence` — English literals were the only
//! way to ask for a fan-out.
//!
//! [`evidence`]: super::evidence
//! [`metadata`]: super::metadata

use runtime::RouteShapeKind;

/// A needle joining two substrings that must BOTH appear for the needle to
/// match ("분할+병렬"), in any order, anywhere in the haystack.
const SHAPE_WORD_CONJUNCTION: char = '+';

/// The requested-shape catalog: ordered rows of `(shape, its words)`, matched
/// as lowercase substrings (`to_ascii_lowercase` leaves CJK and accented Latin
/// alone, so the non-English rows are written exactly as people type them).
///
/// ORDER IS PRECEDENCE — the first row whose words match wins, so the narrow
/// compounds sit above the words they contain: `Solo` (an explicit refusal to
/// delegate) outranks everything, and `ParallelRepairLoop` outranks both of
/// the shapes whose words it is built from.
///
/// AMBIGUOUS WORDS CARRY THEIR CO-WORD AS DATA, not as a branch: 「분할」 on
/// its own is a file split (`분할해줘` = "split this file"), and only 분할
/// standing next to 병렬/동시/갈래 is a request for lanes. Every such word is
/// a `+` conjunction in the row it belongs to, so adding a sixth language adds
/// rows here and nothing else. English "split" is deliberately NOT a
/// conjunction: it is the shipped contract that a bare "split" requests lanes,
/// and the fan-out corpus is written against it.
pub(super) const ROUTE_SHAPE_WORDS: &[(RouteShapeKind, &[&str])] = &[
    (
        RouteShapeKind::Solo,
        &[
            // en
            "solo",
            "no subagent",
            "no agent",
            "main agent only",
            "do not delegate",
            "don't delegate",
            // ko
            "혼자",
            "직접 해",
            "에이전트 없이",
            // ja
            "一人で",
            "自分で",
            "エージェントなし",
            // zh
            "自己做",
            "独自",
            "不用子代理",
            // es
            "sin agentes",
            "sin subagentes",
        ],
    ),
    (
        RouteShapeKind::ParallelRepairLoop,
        &[
            // en
            "parallel repair",
            "parallel_repair_loop",
            // ko
            "병렬+수정 루프",
            "병렬+고칠 때까지",
            // ja
            "並列+修正ループ",
            "並行+修正ループ",
            // zh
            "并行+修复循环",
            // es
            "paralel+reparación",
        ],
    ),
    (
        RouteShapeKind::RepairLoop,
        &[
            // en
            "repair loop",
            "repair_loop",
            "fix_until_verified",
            "fix until verified",
            // ko
            "수정 루프",
            "고칠 때까지",
            "통과할 때까지",
            // ja
            "修正ループ",
            "直るまで",
            "通るまで",
            // zh
            "修复循环",
            "直到通过",
            // es
            "bucle de reparación",
            "hasta que pase",
        ],
    ),
    (
        RouteShapeKind::ParallelLanes,
        &[
            // en
            "parallel",
            "fanout",
            "fan-out",
            "split",
            "separate lanes",
            "worktrees",
            // ko — 분할 needs a co-word; 병렬/팬아웃 stand alone.
            "병렬",
            "팬아웃",
            "분할+동시",
            "분할+갈래",
            // ja — 分割 needs a co-word, the same way 분할 does.
            "並列",
            "並行",
            "ファンアウト",
            "分割+同時",
            // zh
            "并行",
            "拆分+同时",
            "分割+同时",
            // es — the stem covers paralelo/paralela/paralelamente.
            "paralel",
        ],
    ),
    (
        RouteShapeKind::SequentialWorkflow,
        &[
            // en
            "sequential",
            "serialize",
            "plan implement verify",
            // ko
            "순차",
            "차례대로",
            "순서대로",
            // ja
            "順次",
            "順番に",
            "逐次",
            // zh
            "依次",
            "串行",
            "顺序执行",
            // es
            "secuencial",
            "en serie",
        ],
    ),
    (
        RouteShapeKind::OneSpecialist,
        &[
            // en
            "one specialist",
            "delegate ",
            "single specialist",
            "one verifier",
            "single verifier",
            // ko
            "전문가 한 명",
            "검증자 한 명",
            // ja
            "専門家一人",
            "一人の専門家",
            // zh
            "一个专家",
            "一个验证者",
            // es
            "un especialista",
            "un verificador",
        ],
    ),
    // Housekeeping is Solo only after every authored delegation shape had
    // its chance. The earlier Solo row remains an explicit veto on lanes.
    (RouteShapeKind::Solo, &[
        "커밋", "푸시", "정리", "정돈", "commit", "push", "cleanup",
        "コミット", "プッシュ", "整理", "提交", "推送", "清理", "limpiar", "limpieza", "enviar cambios",
    ]),
];

/// Authored lane markers a fan-out prompt carries by convention. Read only
/// under `RouteAutoClassifierMode::Assisted` — the operator's explicit opt-in
/// that their prompts carry markers — and by the marker lane COUNTER, which
/// must look for the same three strings the shape reader does.
pub(super) const ASSISTED_LANE_MARKERS: &[&str] = &["lanes:", "workstreams:", "tracks:"];

/// The shape a person's own words request, or `None` when they asked for no
/// particular shape.
///
/// Two haystacks, because they answer to different owners: `requested` is what
/// the person/parent said this unit IS (name, type, description), while
/// `authored` additionally folds in the brief text — read ONLY for the
/// assisted markers, whose whole contract is "my fan-out prompts carry
/// markers". Natural-language words are read from `requested` alone so a
/// sentence QUOTED inside a lane's brief cannot flip that lane's own shape.
pub(super) fn requested_route_shape(
    requested: &str,
    authored: &str,
    assisted: bool,
) -> Option<RouteShapeKind> {
    ROUTE_SHAPE_WORDS
        .iter()
        .find(|(kind, words)| {
            matches_any(requested, words)
                || (assisted
                    && *kind == RouteShapeKind::ParallelLanes
                    && matches_any(authored, ASSISTED_LANE_MARKERS))
        })
        .map(|(kind, _)| *kind)
}

/// Whether `haystack` carries the words of one shape — for a reader that has
/// its own meaning for them (complexity, not a request).
pub(super) fn text_carries_shape_words(kind: RouteShapeKind, haystack: &str) -> bool {
    ROUTE_SHAPE_WORDS
        .iter()
        .filter(|(row, _)| *row == kind)
        .any(|(_, words)| matches_any(haystack, words))
}

fn matches_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| matches_needle(haystack, needle))
}

/// A plain needle is a substring; a `+` needle is every one of its parts.
fn matches_needle(haystack: &str, needle: &str) -> bool {
    needle
        .split(SHAPE_WORD_CONJUNCTION)
        .all(|part| haystack.contains(part))
}

#[cfg(test)]
mod execution_quality_tests {
    use super::*;
    #[test]
    fn explicit_delegate_request_survives_the_small_task_guard() {
        assert!(super::super::turn::assess_turn_orchestration("Delegate the survey").user_requested_delegation());
        assert!(!super::super::turn::assess_turn_orchestration("Do not delegate; commit and cleanup").user_requested_delegation());
    }

    #[test]
    fn housekeeping_requests_solo_unless_lanes_are_requested() {
        for text in ["커밋 푸시 후 정리", "commit push cleanup", "コミット プッシュ 整理", "提交 推送 清理", "commit enviar limpiar"] {
            assert_eq!(requested_route_shape(text, text, false), Some(RouteShapeKind::Solo), "{text}");
            let lanes = format!("{text} parallel");
            assert_eq!(requested_route_shape(&lanes, &lanes, false), Some(RouteShapeKind::ParallelLanes));
        }
        assert_eq!(requested_route_shape("solo commit parallel", "", false), Some(RouteShapeKind::Solo));
    }
}
