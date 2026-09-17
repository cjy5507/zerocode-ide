use std::collections::BTreeSet;

use runtime::{
    RouteConfidence, RouteContextNeed, RouteDiversityNeed, RouteOutputNeed, RouteRole,
    RouteShapeKind, RouteSignalSource, RouteTaskComplexity, RouteTaskKind, RouteTaskRisk,
    RouteToolNeed, RouteVerificationNeed,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RouteSignal {
    pub source: RouteSignalSource,
    pub key: String,
    pub value: String,
    pub weight: i32,
}

impl RouteSignal {
    fn new(source: RouteSignalSource, key: &str, value: impl Into<String>, weight: i32) -> Self {
        Self { source, key: key.to_string(), value: value.into(), weight }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskRouteMetadata {
    pub kind: RouteTaskKind,
    pub fallback_role: RouteRole,
    pub risk: RouteTaskRisk,
    pub complexity: RouteTaskComplexity,
    pub context_need: RouteContextNeed,
    pub tool_need: RouteToolNeed,
    pub output_need: RouteOutputNeed,
    pub verification_need: RouteVerificationNeed,
    pub diversity_need: RouteDiversityNeed,
    pub confidence: RouteConfidence,
    pub signals: Vec<RouteSignal>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TaskMetadataInput<'a> {
    pub subagent_type: Option<&'a str>,
    pub description: &'a str,
    pub prompt: &'a str,
    pub has_schema: bool,
    pub workflow_member: bool,
}

impl<'a> TaskMetadataInput<'a> {
    pub(super) fn new(subagent_type: Option<&'a str>, description: &'a str, prompt: &'a str) -> Self {
        Self { subagent_type, description, prompt, has_schema: false, workflow_member: false }
    }

    pub(super) fn with_schema(mut self, has_schema: bool) -> Self {
        self.has_schema = has_schema;
        self
    }

    pub(super) fn with_workflow_member(mut self, workflow_member: bool) -> Self {
        self.workflow_member = workflow_member;
        self
    }
}

pub(super) fn classify_task_metadata(input: &TaskMetadataInput<'_>, fallback_role: RouteRole) -> TaskRouteMetadata {
    let mut signals = Vec::new();
    let subagent = input.subagent_type.unwrap_or_default().to_ascii_lowercase();
    let description = input.description.to_ascii_lowercase();
    let prompt = input.prompt.to_ascii_lowercase();
    let haystack = metadata_haystack(input);

    collect_base_signals(input, &subagent, &haystack, &mut signals);
    push_keyword_signals(&mut signals, RouteSignalSource::DescriptionKeyword, &description);
    push_keyword_signals(&mut signals, RouteSignalSource::PromptKeyword, &prompt);

    let risk = risk_from_haystack(&haystack);
    let complexity = complexity_from_haystack(&haystack, fallback_role);
    let kind = score_task_kind(fallback_role, &signals);

    TaskRouteMetadata {
        kind,
        fallback_role,
        risk,
        complexity,
        context_need: context_need_from_haystack(&haystack),
        tool_need: tool_need_from_haystack(&haystack),
        output_need: output_need_from_input(input, fallback_role, &haystack),
        verification_need: verification_need_from_role(fallback_role, &haystack),
        diversity_need: diversity_need_from_role(fallback_role, risk),
        confidence: confidence_from_signals(complexity, &signals),
        signals,
    }
}

/// Fuse a routing-probe self-assessment over the deterministic verdict
/// (`smart.autoClassifier: "probed"`). The bounded arithmetic lives in
/// `runtime::fuse_probe_assessment` (±1 complexity band, risk only rises);
/// this layer records the probe as a first-class [`RouteSignal`] so an
/// agreeing probe legitimately raises route confidence, and recomputes the
/// scored confidence over the fused complexity.
pub(super) fn apply_probe_to_metadata(
    metadata: &mut TaskRouteMetadata,
    probe: runtime::ProbeAssessment,
) -> bool {
    if matches!(probe.confidence, RouteConfidence::Low) {
        return false;
    }
    let fusion = runtime::fuse_probe_assessment(
        metadata.complexity,
        metadata.risk,
        runtime::RouteTaskIntent::Other,
        &probe,
    );
    metadata.signals.push(RouteSignal::new(
        RouteSignalSource::SelfAssessment,
        "probe",
        format!(
            "{:?}·{:?}·{:?}",
            probe.complexity, probe.risk, probe.confidence
        )
        .to_ascii_lowercase(),
        70,
    ));
    metadata.complexity = fusion.complexity;
    metadata.risk = fusion.risk;
    metadata.confidence = confidence_from_signals(metadata.complexity, &metadata.signals);
    true
}

/// Apply the learned complexity calibration (one-band floor promotion for a
/// failing (role, complexity) class) at the same altitude as probe fusion:
/// the scored confidence is recomputed over the promoted complexity, so
/// every downstream consumer sees a confidence that matches the final band.
/// Returns whether a promotion happened (for the audit note).
pub(super) fn apply_calibration_to_metadata(
    metadata: &mut TaskRouteMetadata,
    calibration: &runtime::ComplexityCalibration,
    role_label: &str,
) -> bool {
    let calibrated = calibration.calibrated_complexity(role_label, metadata.complexity);
    if calibrated == metadata.complexity {
        return false;
    }
    metadata.complexity = calibrated;
    metadata.confidence = confidence_from_signals(metadata.complexity, &metadata.signals);
    true
}

fn collect_base_signals(
    input: &TaskMetadataInput<'_>,
    subagent: &str,
    haystack: &str,
    signals: &mut Vec<RouteSignal>,
) {
    if !subagent.is_empty() {
        signals.push(RouteSignal::new(RouteSignalSource::SubagentType, "subagent_type", subagent.to_string(), 80));
    }
    if input.has_schema {
        signals.push(RouteSignal::new(RouteSignalSource::ToolSchema, "schema", "structured-output", 60));
    }
    if input.workflow_member {
        signals.push(RouteSignal::new(RouteSignalSource::WorkflowContext, "workflow_member", "true", 40));
    }
    if contains_any(haystack, &["user asked", "user requested", "explicitly asked"]) {
        signals.push(RouteSignal::new(RouteSignalSource::UserDirective, "user_directive", "explicit", 50));
    }
}

// The haystack keyword tables below carry Korean parity terms next to their
// English originals (matching `infer_route_role` and
// `verification_need_from_role`). Complexity is the load-bearing one: the
// Default role difficulty-routes on it, so a Korean "whole repo" task that
// falls through to `Small` would be sent to a Fast-tier model.
fn risk_from_haystack(haystack: &str) -> RouteTaskRisk {
    if contains_any(haystack, &["critical", "data loss", "unsafe external", "데이터 손실", "치명적"]) {
        RouteTaskRisk::Critical
    } else if contains_any(
        haystack,
        &[
            "credential", "secret", "token", "permission", "sandbox", "auth", "delete", "destructive",
            "자격증명", "시크릿", "비밀키", "권한", "삭제", "파괴적",
        ],
    ) {
        RouteTaskRisk::High
    } else if contains_any(haystack, &["provider", "workflow", "merge", "apply patch", "security", "보안", "병합", "머지"]) {
        RouteTaskRisk::Medium
    } else {
        RouteTaskRisk::Low
    }
}

/// A keyword-less brief this long (chars, so CJK counts fairly) is itself a
/// difficulty signal: nobody writes a multi-paragraph spec for a trivial
/// lookup. Without it, a hard-but-atypically-phrased task floors at `Small`
/// and the Default role difficulty-routes it to a Fast-tier model.
const LONG_BRIEF_COMPLEXITY_CHARS: usize = 800;

/// Implementation markers that, coexisting with a typo mention, mean the
/// task is real work with a typo on the side — the Medium branch's table
/// minus the bare fix-verbs ("fix"/"수정"/"고쳐"), which legitimately
/// describe the typo fix itself.
const TYPO_COEXISTING_WORK_MARKERS: &[&str] = &[
    "implement", "refactor", "edit", "patch", "debug", "workflow", "provider", "contract",
    "구현", "변경", "패치", "마이그레이션", "리팩터링", "리팩토링", "디버깅", "디버그",
];

/// Source-like extensions used only as structural scope evidence. Context
/// files such as `SPEC.md` and settings are intentionally absent: reading a
/// spec does not turn a one-file implementation into a multi-file task.
const IMPLEMENTATION_FILE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cpp", "cs", "css", "go", "h", "hpp", "html", "java", "js", "jsx",
    "kt", "kts", "php", "proto", "py", "rb", "rs", "scala", "sh", "sql", "svelte",
    "swift", "ts", "tsx", "vue",
];

fn implementation_target_from_token(token: &str) -> Option<String> {
    let candidate = token.trim_matches(|ch: char| {
        matches!(
            ch,
            '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';'
        )
    });
    let (stem, suffix) = candidate.rsplit_once('.')?;
    if stem.is_empty() || stem.contains("://") {
        return None;
    }
    // Korean particles can directly follow an ASCII extension
    // (`service.py를`). Keep only the extension prefix in that case.
    let extension = suffix
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>();
    IMPLEMENTATION_FILE_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
        .then(|| format!("{stem}.{}", extension.to_ascii_lowercase()))
}

pub(super) fn implementation_target_count(haystack: &str) -> usize {
    haystack
        .split_whitespace()
        .filter_map(implementation_target_from_token)
        .collect::<BTreeSet<_>>()
        .len()
}

fn has_dependent_multi_target_scope(haystack: &str) -> bool {
    let target_count = implementation_target_count(haystack);
    let names_multiple_targets = target_count >= 2
        || contains_any(
            haystack,
            &["multi file", "multiple files", "여러 파일", "멀티파일", "멀티 파일"],
        );
    let dependent = contains_any(
        haystack,
        &[
            "dependent", "depends on", "interdependent", "tightly coupled", "cross-boundary",
            "서로 의존", "의존 관계", "연동", "결합",
        ],
    );

    // Four explicitly named source targets are already a broad implementation
    // surface. For two or three, require a dependency cue so a mechanical
    // scoped edit does not unlock the premium coding tier.
    target_count >= 4 || (names_multiple_targets && dependent)
}

/// The implementation-verb table that promotes a task to `Medium`. Extracted
/// so [`task_complexity_verb_matched`] can answer "did a VERB put this turn in
/// the Medium band, or only its LENGTH?" without a second, drifting copy of
/// the list.
fn haystack_has_complexity_verb(haystack: &str) -> bool {
    contains_any(
        haystack,
        &[
            "implement", "fix", "edit", "patch", "debug", "refactor", "workflow", "provider", "contract",
            "구현", "수정", "고쳐", "변경", "패치", "마이그레이션", "리팩터링", "리팩토링", "디버깅", "디버그",
        ],
    )
}

/// The exact haystack [`classify_task_metadata`] classifies. Single definition
/// so a sibling question about the same task ([`task_complexity_verb_matched`])
/// is asked of the same text, in the same normalization.
fn metadata_haystack(input: &TaskMetadataInput<'_>) -> String {
    format!(
        "{} {} {}",
        input.subagent_type.unwrap_or_default().to_ascii_lowercase(),
        input.description.to_ascii_lowercase(),
        input.prompt.to_ascii_lowercase(),
    )
}

/// Whether an implementation VERB (not the long-brief length rule) is what put
/// this task in the `Medium` complexity band.
///
/// `complexity_from_haystack` collapses two very different Mediums into one
/// value: "fix the auth module" (a verb matched) and an 800+ character brief
/// with no verb at all (length-only promotion — where a long design brief
/// lands). The probe gate wants to tell them apart, and the complexity value
/// alone cannot. Deliberately a sibling rather than a new return type:
/// `complexity_from_haystack`'s signature has many consumers and none of them
/// need this.
pub(super) fn task_complexity_verb_matched(input: &TaskMetadataInput<'_>) -> bool {
    haystack_has_complexity_verb(&metadata_haystack(input))
}

fn complexity_from_haystack(
    haystack: &str,
    fallback_role: RouteRole,
) -> RouteTaskComplexity {
    if haystack.trim().is_empty() {
        RouteTaskComplexity::Unknown
    } else if contains_any(
        haystack,
        &[
            "whole repo", "multi subsystem",
            "레포 전체", "저장소 전체", "코드베이스 전체", "전체 코드베이스", "대규모", "모든 모듈", "여러 서브시스템",
            // A goal-pivot turn: the goal controller stamps this marker on a
            // forced re-approach after a stall — a problem that already
            // exhausted the current model's approach deserves the strong tier.
            "[zo:goal-pivot]",
        ],
    ) || (matches!(fallback_role, RouteRole::Coding | RouteRole::Debugging)
        && has_dependent_multi_target_scope(haystack))
        || (!matches!(fallback_role, RouteRole::Coding | RouteRole::Debugging)
        && (super::shape_words::text_carries_shape_words(RouteShapeKind::ParallelLanes, haystack)
            || contains_any(haystack, &["integration", "통합"])))
    {
        RouteTaskComplexity::Large
    } else if (super::infer::contains_ascii_word(haystack, "typo")
        || contains_any(haystack, &["오타"]))
        && !contains_any(haystack, TYPO_COEXISTING_WORK_MARKERS)
    {
        // Checked BEFORE the implementation verbs: "fix the typo" is
        // quintessentially trivial even though it says "fix" — the verb
        // branch used to win and over-provision a Medium-tier model (caught
        // by the complexity evaluation corpus). Guarded the other way too:
        // a typo mention COEXISTING with real work ("refactor the auth
        // module and fix a typo") must not demote that work to Trivial, so
        // any implementation marker beyond the bare fix-verbs falls through
        // to the Medium branch below. Word-boundary for "typo" so
        // "Typography" cannot demote a real refactor; the Korean marker is a
        // substring on purpose (no-space compounds like "오타수정").
        RouteTaskComplexity::Trivial
    } else if haystack_has_complexity_verb(haystack) {
        RouteTaskComplexity::Medium
    } else if super::infer::haystack_has_failing_test_evidence(haystack) {
        // A failing named test run is implementation work whatever verb (or
        // none) the person used — checked before the docs/label class so a
        // "docstring" or "the docs warn against" inside such a task cannot
        // demote it (r50 tasks 13/15 read Trivial that way).
        RouteTaskComplexity::Medium
    } else if contains_any(haystack, &["label", "copy", "라벨"])
        || super::infer::contains_ascii_word(haystack, "docs")
    {
        RouteTaskComplexity::Trivial
    } else if haystack.chars().count() >= LONG_BRIEF_COMPLEXITY_CHARS {
        RouteTaskComplexity::Medium
    } else {
        RouteTaskComplexity::Small
    }
}

fn context_need_from_haystack(haystack: &str) -> RouteContextNeed {
    if haystack.trim().is_empty() {
        RouteContextNeed::Unknown
    } else if contains_any(
        haystack,
        &["whole repo", "multi subsystem", "레포 전체", "저장소 전체", "코드베이스 전체", "전체 코드베이스", "모든 모듈", "여러 서브시스템"],
    ) {
        RouteContextNeed::WholeRepo
    } else if implementation_target_count(haystack) >= 2
        || contains_any(haystack, &["multi file", "integration", "contract", "여러 파일", "멀티파일", "멀티 파일", "통합"])
    {
        RouteContextNeed::MultiFile
    } else if contains_any(haystack, &["file", "code", "implement", "fix", "edit", "patch", "debug", "구현", "수정", "고쳐", "변경", "패치", "파일", "코드"]) {
        RouteContextNeed::LocalFiles
    } else {
        RouteContextNeed::None
    }
}

fn tool_need_from_haystack(haystack: &str) -> RouteToolNeed {
    if haystack.trim().is_empty() {
        RouteToolNeed::Unknown
    } else if contains_any(haystack, &["network", "web", "url", "http", "네트워크", "웹"]) {
        RouteToolNeed::Network
    } else if contains_any(haystack, &["run tests", "test suite", "shell", "command", "테스트 실행", "테스트 돌려", "셸", "쉘", "명령어"]) {
        RouteToolNeed::Shell
    } else if contains_any(haystack, &["fix", "implement", "edit", "patch", "modify", "구현", "수정", "고쳐", "변경", "패치"]) {
        RouteToolNeed::Write
    } else if contains_any(haystack, &["review", "verify", "read", "inspect", "리뷰", "검토", "검증", "조사"]) {
        RouteToolNeed::ReadOnly
    } else {
        RouteToolNeed::None
    }
}

fn output_need_from_input(input: &TaskMetadataInput<'_>, fallback_role: RouteRole, haystack: &str) -> RouteOutputNeed {
    if input.has_schema {
        RouteOutputNeed::Structured
    } else if matches!(fallback_role, RouteRole::Verifier) || contains_any(haystack, &["test evidence", "run tests"]) {
        RouteOutputNeed::TestEvidence
    } else if contains_any(haystack, &["patch", "fix", "implement", "edit", "modify", "구현", "수정", "고쳐", "변경", "패치"]) {
        RouteOutputNeed::Patch
    } else {
        RouteOutputNeed::FreeText
    }
}

fn verification_need_from_role(fallback_role: RouteRole, haystack: &str) -> RouteVerificationNeed {
    if haystack.trim().is_empty() {
        RouteVerificationNeed::Unknown
    } else if contains_any(haystack, &["full verification", "test suite", "integration check"]) {
        RouteVerificationNeed::Full
    } else if matches!(fallback_role, RouteRole::Verifier | RouteRole::Reviewer)
        || contains_any(haystack, &["verify", "review", "run tests", "검증", "테스트", "검사", "리뷰", "검토"])
    {
        RouteVerificationNeed::Focused
    } else {
        RouteVerificationNeed::None
    }
}

fn diversity_need_from_role(fallback_role: RouteRole, risk: RouteTaskRisk) -> RouteDiversityNeed {
    if matches!(risk, RouteTaskRisk::High | RouteTaskRisk::Critical) {
        RouteDiversityNeed::Helpful
    } else if matches!(fallback_role, RouteRole::Judge) {
        RouteDiversityNeed::Required
    } else if matches!(fallback_role, RouteRole::Verifier | RouteRole::Reviewer) {
        RouteDiversityNeed::Helpful
    } else {
        RouteDiversityNeed::None
    }
}

/// Scored confidence: the accumulated signal weight, not merely "any signal".
/// This is what makes `RouteSignal::weight` load-bearing — more and stronger
/// signals raise confidence. `Unknown` complexity (empty task text) stays `Low`.
fn confidence_from_signals(complexity: RouteTaskComplexity, signals: &[RouteSignal]) -> RouteConfidence {
    if matches!(complexity, RouteTaskComplexity::Unknown) {
        return RouteConfidence::Low;
    }
    match signals.iter().map(|signal| signal.weight).sum::<i32>() {
        weight if weight >= 100 => RouteConfidence::High,
        weight if weight >= 40 => RouteConfidence::Medium,
        _ => RouteConfidence::Low,
    }
}

/// Scored task-kind classifier. Replaces the flat "kind = role" mapping with a
/// weighted accumulation: the role-derived kind is a strong baseline, and the
/// already-collected signals (subagent type, prompt/description keywords) add
/// their own `weight` to the kind they imply. The highest-scoring kind wins,
/// falling back to the role-derived kind on a tie. This reads `RouteSignal`
/// weights (so they are no longer dead) and lets an explicit strong signal tip
/// the kind, while preserving role-driven classification when signals agree.
fn score_task_kind(fallback_role: RouteRole, signals: &[RouteSignal]) -> RouteTaskKind {
    let role_kind = kind_from_role(fallback_role);
    let mut scores: Vec<(RouteTaskKind, i32)> = vec![(role_kind, 60)];
    let mut add = |kind: RouteTaskKind, weight: i32| {
        if let Some(entry) = scores.iter_mut().find(|(existing, _)| *existing == kind) {
            entry.1 += weight;
        } else {
            scores.push((kind, weight));
        }
    };
    for signal in signals {
        let kind = match signal.source {
            // A subagent type implies a kind via the same role inference.
            RouteSignalSource::SubagentType => Some(kind_from_role(super::infer::infer_route_role(Some(&signal.value), "", ""))),
            _ if signal.key == "keyword" => kind_for_keyword(&signal.value),
            _ => None,
        };
        if let Some(kind) = kind {
            add(kind, signal.weight);
        }
    }
    scores
        .into_iter()
        .max_by(|(a_kind, a), (b_kind, b)| {
            // Higher score wins; on a tie prefer the role-derived kind.
            a.cmp(b)
                .then_with(|| (*a_kind == role_kind).cmp(&(*b_kind == role_kind)))
        })
        .map_or(role_kind, |(kind, _)| kind)
}

/// The task kind a classification keyword implies, or `None` for keywords that
/// only inform risk/complexity (e.g. `security`, `contract`, `parallel`).
fn kind_for_keyword(keyword: &str) -> Option<RouteTaskKind> {
    match keyword {
        "verify" => Some(RouteTaskKind::Verification),
        "review" => Some(RouteTaskKind::Review),
        "debug" => Some(RouteTaskKind::Debugging),
        "research" => Some(RouteTaskKind::Research),
        "implement" | "fix" => Some(RouteTaskKind::Coding),
        "plan" => Some(RouteTaskKind::Analysis),
        _ => None,
    }
}

fn kind_from_role(role: RouteRole) -> RouteTaskKind {
    match role {
        RouteRole::Default => RouteTaskKind::Default,
        RouteRole::Fast => RouteTaskKind::Fast,
        RouteRole::Coding => RouteTaskKind::Coding,
        RouteRole::Debugging => RouteTaskKind::Debugging,
        RouteRole::Verifier => RouteTaskKind::Verification,
        RouteRole::Reviewer => RouteTaskKind::Review,
        RouteRole::Analysis => RouteTaskKind::Analysis,
        RouteRole::Research => RouteTaskKind::Research,
        RouteRole::Writing => RouteTaskKind::Writing,
        RouteRole::Design => RouteTaskKind::Design,
        RouteRole::Judge => RouteTaskKind::Judge,
        RouteRole::Synthesizer => RouteTaskKind::Synthesis,
    }
}

fn push_keyword_signals(signals: &mut Vec<RouteSignal>, source: RouteSignalSource, text: &str) {
    for (keyword, canonical) in [
        ("verify", "verify"),
        ("검증", "verify"),
        ("테스트", "verify"),
        ("review", "review"),
        ("리뷰", "review"),
        ("검토", "review"),
        ("debug", "debug"),
        ("디버그", "debug"),
        ("research", "research"),
        ("조사", "research"),
        ("implement", "implement"),
        ("구현", "implement"),
        ("fix", "fix"),
        ("수정", "fix"),
        ("고쳐", "fix"),
        ("변경", "fix"),
        ("패치", "fix"),
        ("plan", "plan"),
        ("계획", "plan"),
        ("parallel", "parallel"),
        ("contract", "contract"),
        ("security", "security"),
    ] {
        if text.contains(keyword) {
            signals.push(RouteSignal::new(source, "keyword", canonical, 20));
        }
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool { needles.iter().any(|needle| text.contains(needle)) }
