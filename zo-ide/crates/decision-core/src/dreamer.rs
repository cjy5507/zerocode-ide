//! Dreaming memory: the portable, IO-free curation brain.
//!
//! This is the "Dreamer" axis of the Agentic Loop Program: the process
//! that runs *between* sessions, reviews what happened, and promotes only the
//! durable lessons into long-term memory. Like [`crate::deep_lane`], this module
//! is a pure decision policy — it owns *what to promote and why*, never *how to
//! read traces or write files*. The IO seam (trace source, memory writer,
//! scheduler) lives in `runtime::memory::dreamer`, so this logic can be unit
//! tested deterministically and reused by any harness.
//!
//! ## Why a gate, not a sink
//!
//! The single biggest failure mode of session-spanning memory is *pollution*
//! (doc §10-2): one wrong inference from a single session leaks into every
//! future run. So [`curate`] is deliberately conservative — a lesson is
//! promoted only when it is **both repeated across distinct sessions and
//! verified**, within a per-run budget, and every rejection is reported with an
//! explicit [`SkipReason`] so the decision is auditable rather than silent.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// One candidate lesson distilled from a single session's trace.
///
/// The IO layer produces these (one per noteworthy outcome); the brain decides
/// which survive. `signature` is the dedup key — two observations from
/// *different* sessions that share a signature are "the same lesson seen
/// twice", which is exactly the repetition the promotion gate counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LessonObservation {
    /// Stable identity of the lesson, independent of which session saw it.
    /// Observations are grouped by this to count cross-session repetition.
    pub signature: String,
    /// The session this observation came from. Distinct values across a
    /// signature's group are what satisfy [`PromotionPolicy::min_distinct_sessions`].
    pub session_id: String,
    /// Human-readable lesson text (becomes the memory entry body).
    pub lesson: String,
    /// One-line summary (becomes the `MEMORY.md` pointer + entry summary).
    pub summary: String,
    /// What kind of lesson this is — drives slug prefixing and lets callers
    /// filter (e.g. promote preferences eagerly, gotchas conservatively).
    pub kind: LessonKind,
    /// Whether this observation was objectively verified in its session (a
    /// green test gate, an accepted verifier verdict, an explicit user
    /// confirmation). Unverified observations can be *counted* but, under a
    /// strict policy, never promoted alone — the §9-6 "record after
    /// verification" rule.
    pub verified: bool,
}

/// The category of a distilled lesson. Mirrors the doc's `MemoryEntry` intent
/// (§5-7) and decides how cautiously a lesson is promoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LessonKind {
    /// A stable user/project preference (style, format, workflow choice).
    Preference,
    /// A hard-won gotcha or recurring failure mode to avoid.
    Gotcha,
    /// An effective workflow or approach worth repeating.
    Workflow,
    /// A durable project constraint or invariant.
    Constraint,
}

impl LessonKind {
    /// Lowercase token used on slug prefixes and JSON boundaries. Stable across
    /// versions so persisted candidate files keep parsing.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Gotcha => "gotcha",
            Self::Workflow => "workflow",
            Self::Constraint => "constraint",
        }
    }
}

impl std::fmt::Display for LessonKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Coarse source category for a self-improvement candidate. Unlike
/// [`LessonKind`], this does not describe memory to promote; it describes the
/// runtime signal that may deserve a future isolated fix. Keeping it in the pure
/// brain lets the runtime store append-only candidate records without owning the
/// ranking policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// A deep-gate/objective verification accepted a turn.
    VerifiedAccept,
    /// `/goal` reached a terminal state. This records lifecycle telemetry;
    /// it is not evidence that Zo itself needs a repair.
    GoalTerminal,
    /// `/goal` reached a verified failed terminal state. This is distinct from
    /// a successful terminal event so only an actual failed goal may enter
    /// self-repair planning.
    GoalFailure,
    /// A turn failed before normal completion for an actionable reason.
    TurnFailure,
    /// The user intentionally stopped a turn. This is durable operational
    /// telemetry, not a self-repair defect.
    UserCancelled,
    /// A successful foreground turn finished; this is a low-priority scheduler
    /// hint, not evidence of a bug by itself.
    PostTurn,
    /// A harness feature attested as FAILING across the `/refine` evidence
    /// window: it tried at least once, never fired, on the current build.
    /// This is the live fail-open defect signature (the shape a 100%-failing
    /// routing probe had for weeks while emitting nothing), promoted from the
    /// attest ledger into the candidate store so `/improve` can plan a repair
    /// for it — counted by code, not inferred by a model.
    HarnessDefect,
}

impl CandidateKind {
    /// Stable lowercase token for JSON boundaries and deterministic ids.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VerifiedAccept => "verified_accept",
            Self::GoalTerminal => "goal_terminal",
            Self::GoalFailure => "goal_failure",
            Self::TurnFailure => "turn_failure",
            Self::UserCancelled => "user_cancelled",
            Self::PostTurn => "post_turn",
            Self::HarnessDefect => "harness_defect",
        }
    }

    /// Severity prior used by [`candidate_score`]. Failure-shaped signals rank
    /// above success-shaped hints, while post-turn pulses stay deliberately low
    /// so they can drive scheduling without crowding out real failures.
    #[must_use]
    pub const fn base_score(self) -> u32 {
        match self {
            Self::TurnFailure => 90,
            // A failing harness feature outranks a failed goal: the goal
            // failed once, but a Failing attestation means a shipped code
            // path fails EVERY time it is reached and hides it.
            Self::HarnessDefect => 85,
            Self::GoalTerminal | Self::GoalFailure => 80,
            Self::VerifiedAccept => 50,
            Self::PostTurn => 10,
            Self::UserCancelled => 0,
        }
    }

    /// Whether this signal may enter self-repair planning.
    #[must_use]
    pub const fn is_actionable(self) -> bool {
        matches!(
            self,
            Self::TurnFailure | Self::GoalFailure | Self::HarnessDefect
        )
    }
}

impl std::fmt::Display for CandidateKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Lifecycle state for a self-improvement candidate. Phase 3 only creates and
/// ranks proposed records; later patch runners will move them through the other
/// states. Defining the state machine now keeps the append-only log forward
/// compatible without enabling automatic edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Proposed,
    Planned,
    Quarantined,
    Rejected,
    Applied,
}

impl CandidateStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Planned => "planned",
            Self::Quarantined => "quarantined",
            Self::Rejected => "rejected",
            Self::Applied => "applied",
        }
    }

    /// A resolved candidate that must not be re-proposed: `Applied` (the fix
    /// already landed) or `Rejected` (already declined). Ranking drops these so
    /// `/improve` does not regenerate a candidate it just acted on.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Applied | Self::Rejected)
    }
}

impl std::fmt::Display for CandidateStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// One concrete evidence item supporting a self-improvement candidate. It is
/// intentionally terse: enough to dedupe/rank and point an auditor back to the
/// source, but not enough to leak prompts or transcripts into durable state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateEvidence {
    /// Session that produced the signal. Empty strings are allowed only for
    /// host-level startup/scheduler evidence.
    pub session_id: String,
    /// Stable producer name such as `deep_gate`, `goal`, `turn`, or `post_turn`.
    pub source: String,
    /// Sanitized, human-readable detail. Callers must not store raw prompts.
    pub detail: String,
    /// Whether this evidence is backed by an objective controller/check result.
    pub verified: bool,
}

/// A natural-trigger self-improvement candidate. This is separate from
/// [`LessonObservation`]: candidates may later drive planning or quarantined
/// patches, while lessons may be promoted to long-term memory only through the
/// stricter curation gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfImproveCandidate {
    pub id: String,
    pub kind: CandidateKind,
    pub summary: String,
    pub status: CandidateStatus,
    pub evidence: Vec<CandidateEvidence>,
    /// First persisted observation of this logical candidate. Zero means the
    /// record predates timestamped candidate aggregation.
    #[serde(default)]
    pub first_observed_at_ms: u64,
    /// Most recent persisted observation. The runtime supplies this clock data;
    /// decision-core only consumes it through pure ranking functions.
    #[serde(default)]
    pub last_observed_at_ms: u64,
}

impl SelfImproveCandidate {
    /// Build a candidate with a deterministic id derived from its kind and
    /// summary. Non-actionable telemetry is terminal on creation so schedulers
    /// cannot report it as a proposal awaiting repair.
    #[must_use]
    pub fn new(
        kind: CandidateKind,
        summary: impl Into<String>,
        evidence: Vec<CandidateEvidence>,
    ) -> Self {
        let summary = summary.into();
        Self {
            id: self_improve_candidate_id(kind, &summary),
            kind,
            summary,
            status: if kind.is_actionable() {
                CandidateStatus::Proposed
            } else {
                CandidateStatus::Rejected
            },
            evidence,
            first_observed_at_ms: 0,
            last_observed_at_ms: 0,
        }
    }
}

fn slug_body(input: &str) -> String {
    let mut body = String::new();
    let mut previous_dash = true;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            body.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            body.push('-');
            previous_dash = true;
        }
    }
    while body.ends_with('-') {
        body.pop();
    }
    body
}

/// Deterministic self-improvement candidate id. Stable ids are what make the
/// runtime log append-only: a repeated signal appends more evidence under the
/// same logical candidate instead of overwriting prior observations.
#[must_use]
pub fn self_improve_candidate_id(kind: CandidateKind, summary: &str) -> String {
    let body = slug_body(summary);
    if body.is_empty() {
        kind.as_str().to_string()
    } else {
        format!("{}-{body}", kind.as_str())
    }
}

/// Longest raw token `error_signature_label` will consider — a provider error
/// code, never free text.
const SIGNATURE_TOKEN_MAX_LEN: usize = 32;

/// Build a bounded, segmentable failure label from a stable failure class plus
/// salient tokens mined from the raw error message: the first HTTP-style
/// 4xx/5xx status and the first `UPPER_SNAKE` provider/tool error code.
///
/// The label keys candidate identity (via [`self_improve_candidate_id`]), so
/// its cardinality must stay low: class (a fixed ~10-value vocabulary) plus
/// status plus code — free text never flows through. Without this, every turn
/// failure aggregated into one generic "turn failed" candidate whose evidence
/// mixed unrelated root causes, and no advisor or patch generator could act on
/// it.
#[must_use]
pub fn error_signature_label(class: &str, message: &str) -> String {
    let mut label = class.trim().to_string();
    if label.is_empty() {
        label.push_str("unclassified");
    }
    let mut status: Option<&str> = None;
    let mut code: Option<&str> = None;
    for raw in message.split(|c: char| {
        c.is_whitespace() || matches!(c, '(' | ')' | ':' | ',' | ';' | '"' | '\'' | '[' | ']')
    }) {
        let token = raw.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
        if token.is_empty() || token.len() > SIGNATURE_TOKEN_MAX_LEN {
            continue;
        }
        if status.is_none()
            && token.len() == 3
            && token.starts_with(['4', '5'])
            && token.bytes().all(|b| b.is_ascii_digit())
        {
            status = Some(token);
        } else if code.is_none()
            && token.len() >= 4
            && token
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            && token.bytes().any(|b| b.is_ascii_uppercase())
        {
            code = Some(token);
        }
        if status.is_some() && code.is_some() {
            break;
        }
    }
    if let Some(status) = status {
        label.push_str(" · ");
        label.push_str(status);
    }
    if let Some(code) = code {
        label.push_str(if status.is_some() { " " } else { " · " });
        label.push_str(code);
    }
    label
}

const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

fn bounded_count(count: usize, cap: usize) -> u32 {
    u32::try_from(count.min(cap)).unwrap_or(u32::MAX)
}

fn distinct_non_empty<'a>(values: impl Iterator<Item = &'a str>) -> usize {
    values
        .filter(|value| !value.trim().is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

/// Pure priority score for candidate triage. Severity dominates while evidence
/// quality comes from independent sessions, objective verification, and source
/// diversity. Repeating one event in one session cannot saturate the score.
#[must_use]
pub fn candidate_score(candidate: &SelfImproveCandidate) -> u32 {
    if !candidate.kind.is_actionable() {
        return 0;
    }
    let independent_sessions = distinct_non_empty(
        candidate
            .evidence
            .iter()
            .map(|evidence| evidence.session_id.as_str()),
    );
    let verified_sessions = distinct_non_empty(
        candidate
            .evidence
            .iter()
            .filter(|evidence| evidence.verified)
            .map(|evidence| evidence.session_id.as_str()),
    );
    let distinct_sources = distinct_non_empty(
        candidate
            .evidence
            .iter()
            .map(|evidence| evidence.source.as_str()),
    );
    candidate.kind.base_score()
        + bounded_count(independent_sessions, 5) * 8
        + bounded_count(verified_sessions, 3) * 10
        + bounded_count(distinct_sources, 3) * 3
}

fn recency_bonus(candidate: &SelfImproveCandidate, now_ms: u64) -> u32 {
    if candidate.last_observed_at_ms == 0 || now_ms < candidate.last_observed_at_ms {
        return 0;
    }
    match now_ms - candidate.last_observed_at_ms {
        age if age <= DAY_MS => 15,
        age if age <= 7 * DAY_MS => 10,
        age if age <= 30 * DAY_MS => 5,
        _ => 0,
    }
}

/// Score a candidate at an explicit clock value. Keeping the clock outside the
/// policy makes ranking deterministic in tests and reusable by non-runtime hosts.
#[must_use]
pub fn candidate_score_at(candidate: &SelfImproveCandidate, now_ms: u64) -> u32 {
    if !candidate.kind.is_actionable() {
        return 0;
    }
    candidate_score(candidate) + recency_bonus(candidate, now_ms)
}

/// Return candidates sorted by priority without mutating the caller's
/// collection. The explicit clock keeps recency deterministic.
#[must_use]
pub fn rank_self_improve_candidates_at(
    candidates: &[SelfImproveCandidate],
    now_ms: u64,
) -> Vec<SelfImproveCandidate> {
    let mut ranked = candidates.to_vec();
    ranked.sort_by(|a, b| {
        candidate_score_at(b, now_ms)
            .cmp(&candidate_score_at(a, now_ms))
            .then_with(|| b.last_observed_at_ms.cmp(&a.last_observed_at_ms))
            .then_with(|| a.id.cmp(&b.id))
    });
    ranked
}

/// Deterministic ranking for callers without a clock. The newest timestamp in
/// the input becomes the reference point.
#[must_use]
pub fn rank_self_improve_candidates(
    candidates: &[SelfImproveCandidate],
) -> Vec<SelfImproveCandidate> {
    let reference_ms = candidates
        .iter()
        .map(|candidate| candidate.last_observed_at_ms)
        .max()
        .unwrap_or(0);
    rank_self_improve_candidates_at(candidates, reference_ms)
}

/// Select a bounded, deterministic set of representative evidence. Objective
/// evidence wins, then independent sessions and stable lexical order. Report
/// formatting remains a runtime responsibility.
#[must_use]
pub fn representative_candidate_evidence(
    candidate: &SelfImproveCandidate,
    limit: usize,
) -> Vec<&CandidateEvidence> {
    if limit == 0 {
        return Vec::new();
    }
    let mut evidence: Vec<&CandidateEvidence> = candidate.evidence.iter().collect();
    evidence.sort_by(|a, b| {
        b.verified
            .cmp(&a.verified)
            .then_with(|| a.session_id.cmp(&b.session_id))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.detail.cmp(&b.detail))
    });
    let mut buckets = std::collections::BTreeSet::new();
    let mut selected = Vec::with_capacity(limit.min(evidence.len()));
    for item in evidence {
        let session = item.session_id.trim();
        let bucket = if session.is_empty() {
            format!("host:{}", item.source.trim())
        } else {
            format!("session:{session}")
        };
        if !buckets.insert(bucket) {
            continue;
        }
        selected.push(item);
        if selected.len() == limit {
            break;
        }
    }
    selected
}

/// Read-only advisory role used by native `DreamFusion` v0. Advisors are schema
/// records, not executors: they do not own tools, patches, or side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvisorRole {
    RootCause,
    Risk,
    TestPlan,
    AlternativeHypothesis,
}

impl AdvisorRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RootCause => "root_cause",
            Self::Risk => "risk",
            Self::TestPlan => "test_plan",
            Self::AlternativeHypothesis => "alternative_hypothesis",
        }
    }
}

impl std::fmt::Display for AdvisorRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchRisk {
    Low,
    Medium,
    High,
}

impl PatchRisk {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

impl std::fmt::Display for PatchRisk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvisorFinding {
    pub role: AdvisorRole,
    pub candidate_id: String,
    pub summary: String,
    pub confidence: f32,
    pub risk: PatchRisk,
    pub recommended_checks: Vec<String>,
    /// Whether this advisor believes the candidate is actionable enough for a
    /// later quarantined patch proposal. This is advisory only.
    pub accepts_quarantine: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamJudgeDecision {
    PlanPatch,
    Quarantine,
    NeedMoreEvidence,
    Reject,
}

impl DreamJudgeDecision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlanPatch => "plan_patch",
            Self::Quarantine => "quarantine",
            Self::NeedMoreEvidence => "need_more_evidence",
            Self::Reject => "reject",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DreamFusionReport {
    pub run_id: String,
    pub candidate_id: String,
    pub summary: String,
    pub decision: DreamJudgeDecision,
    pub risk: PatchRisk,
    pub findings: Vec<AdvisorFinding>,
    pub required_checks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchCheckResult {
    pub name: String,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantinePatchRun {
    pub run_id: String,
    pub candidate_id: String,
    pub base_commit: String,
    /// SHA-256 of the exact unified diff submitted to quarantine.
    pub patch_digest: String,
    pub changed_paths: Vec<String>,
    pub check_results: Vec<PatchCheckResult>,
    pub risk: PatchRisk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent gate precondition, not a state-machine mode"
)]
pub struct ApplyGateInput {
    pub approved_by_user: bool,
    pub clean_tree: bool,
    pub base_commit_matches: bool,
    pub paths_allowed: bool,
    pub focused_checks_green: bool,
    pub reviewer_accepted: bool,
    pub risk: PatchRisk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyGateDecision {
    pub eligible: bool,
    pub reasons: Vec<String>,
}

#[must_use]
pub fn synthesize_dream_fusion(
    run_id: &str,
    candidate: &SelfImproveCandidate,
    mut findings: Vec<AdvisorFinding>,
) -> DreamFusionReport {
    findings.sort_by(|a, b| a.role.cmp(&b.role));
    let risk = findings
        .iter()
        .map(|finding| finding.risk)
        .max()
        .unwrap_or(PatchRisk::Medium);
    let accepted = findings
        .iter()
        .filter(|finding| finding.accepts_quarantine)
        .count();
    let required_checks = collect_required_checks(&findings);
    let decision = if findings.is_empty() || required_checks.is_empty() {
        DreamJudgeDecision::NeedMoreEvidence
    } else if risk == PatchRisk::High {
        DreamJudgeDecision::Quarantine
    } else if accepted * 2 >= findings.len() {
        DreamJudgeDecision::PlanPatch
    } else {
        DreamJudgeDecision::NeedMoreEvidence
    };

    DreamFusionReport {
        run_id: run_id.to_string(),
        candidate_id: candidate.id.clone(),
        summary: candidate.summary.clone(),
        decision,
        risk,
        findings,
        required_checks,
    }
}

fn collect_required_checks(findings: &[AdvisorFinding]) -> Vec<String> {
    let mut checks = Vec::new();
    for finding in findings {
        for check in &finding.recommended_checks {
            let check = check.trim();
            if !check.is_empty() && !checks.iter().any(|existing| existing == check) {
                checks.push(check.to_string());
            }
        }
    }
    checks
}

#[must_use]
pub fn decide_apply_gate(input: &ApplyGateInput) -> ApplyGateDecision {
    let mut reasons = Vec::new();
    if !input.approved_by_user {
        reasons.push("missing_user_approval".to_string());
    }
    if !input.clean_tree {
        reasons.push("worktree_not_clean".to_string());
    }
    if !input.base_commit_matches {
        reasons.push("base_commit_mismatch".to_string());
    }
    if !input.paths_allowed {
        reasons.push("path_not_allowlisted".to_string());
    }
    if !input.focused_checks_green {
        reasons.push("focused_checks_not_green".to_string());
    }
    if !input.reviewer_accepted {
        reasons.push("reviewer_not_accepted".to_string());
    }
    if input.risk == PatchRisk::High {
        reasons.push("high_risk_patch".to_string());
    }
    ApplyGateDecision {
        eligible: reasons.is_empty(),
        reasons,
    }
}

/// The brakes on promotion (doc §9-7 "loops need a budget and a brake", §10-2
/// "contaminated memory" defenses). Every field tightens the gate; the
/// [`Default`] is the conservative production policy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PromotionPolicy {
    /// How many *distinct* sessions must report a signature before it is
    /// durable rather than a one-off. Must be `>= 1`; `1` disables the
    /// repetition requirement.
    pub min_distinct_sessions: usize,
    /// When true, a lesson is promoted only if at least one of its
    /// observations was [`LessonObservation::verified`]. The §9-6 rule.
    pub require_verified: bool,
    /// Upper bound on promotions in a single run, so a noisy backlog cannot
    /// flood memory in one pass (the §9-7 budget brake). `0` means unlimited.
    pub max_promotions_per_run: usize,
    /// Lessons whose computed [`confidence`](PromotedLesson::confidence) is
    /// below this are skipped. Range `0.0..=1.0`.
    pub confidence_floor: f32,
    /// Days until a promoted lesson should be revisited/expired, stamped onto
    /// the entry so stale low-value lessons decay (doc §5-7 `expiry`, §10-2
    /// expiry policy). `0` means no expiry.
    pub expiry_days: u32,
}

impl Default for PromotionPolicy {
    fn default() -> Self {
        // Conservative production defaults: a lesson must be seen in two
        // distinct sessions AND be verified before it is ever written, at most
        // five per run, and only above moderate confidence.
        Self {
            min_distinct_sessions: 2,
            require_verified: true,
            max_promotions_per_run: 5,
            confidence_floor: 0.5,
            expiry_days: 90,
        }
    }
}

impl PromotionPolicy {
    /// A permissive policy for tests/ad-hoc runs: promote anything seen once,
    /// verified or not, with no budget. Never use as a production default.
    #[must_use]
    pub const fn permissive() -> Self {
        Self {
            min_distinct_sessions: 1,
            require_verified: false,
            max_promotions_per_run: 0,
            confidence_floor: 0.0,
            expiry_days: 0,
        }
    }
}

/// Why a candidate signature was not promoted. Carried in the plan so the run
/// is auditable (doc §13 "final result returns with evidence and remaining
/// risk") instead of silently dropping observations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// Seen in fewer distinct sessions than the policy requires.
    NotRepeatedEnough {
        distinct_sessions: usize,
        required: usize,
    },
    /// Policy requires verification and no observation was verified.
    Unverified,
    /// Confidence fell below [`PromotionPolicy::confidence_floor`].
    BelowConfidenceFloor { confidence: f32, floor: f32 },
    /// Already present in long-term memory under this slug (idempotent re-run).
    AlreadyKnown,
    /// Another approved lesson with the same normalized summary template is
    /// the representative promotion for this curation pass.
    SameSummaryTemplate { promoted_slug: String },
    /// The per-run promotion budget was exhausted before this candidate.
    BudgetExhausted,
}

/// A lesson the gate approved for writing to long-term memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotedLesson {
    /// Kebab-case identity derived from the signature; the memory entry's slug.
    pub slug: String,
    /// One-line `MEMORY.md` pointer summary.
    pub summary: String,
    /// Full lesson body for the entry file.
    pub lesson: String,
    /// Lesson category.
    pub kind: LessonKind,
    /// Number of distinct sessions that supported this lesson.
    pub distinct_sessions: usize,
    /// Whether any supporting observation was verified.
    pub verified: bool,
    /// Evidence-derived confidence in `0.0..=1.0`.
    pub confidence: f32,
    /// Days-to-expiry stamped from the policy (`0` = none).
    pub expiry_days: u32,
}

/// A skipped candidate plus the reason, for the audit trail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkippedLesson {
    pub slug: String,
    pub summary: String,
    pub reason: SkipReason,
}

/// The full, deterministic output of one curation pass: what to write and what
/// was rejected (with reasons). Contains no IO — the caller applies it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CurationPlan {
    /// Lessons approved for promotion, in deterministic order (confidence desc,
    /// then slug asc), already truncated to the policy budget.
    pub promote: Vec<PromotedLesson>,
    /// Candidates that did not pass, each with its [`SkipReason`].
    pub skipped: Vec<SkippedLesson>,
}

impl CurationPlan {
    /// True when the pass approved nothing — the caller can short-circuit all
    /// IO (no file writes, no index rewrite).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.promote.is_empty()
    }
}

/// Derive a kebab-case memory slug from a free-form signature, prefixed by the
/// lesson kind so related lessons cluster in the index (e.g.
/// `gotcha-sqlite-busy-timeout`). Pure and deterministic: the same signature
/// always yields the same slug, which is what makes re-runs idempotent against
/// existing memory.
#[must_use]
pub fn slug_for(kind: LessonKind, signature: &str) -> String {
    // Kebab-case the signature body on its own, then join to the kind prefix
    // with a single dash. Building the body separately avoids fusing the prefix
    // into the first signature segment (`gotcha` + `sqlite` → `gotcha-sqlite`,
    // never `gotchasqlite`).
    let mut body = String::new();
    let mut previous_dash = true; // suppress a leading dash on the body
    for ch in signature.chars() {
        if ch.is_ascii_alphanumeric() {
            body.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            body.push('-');
            previous_dash = true;
        }
    }
    while body.ends_with('-') {
        body.pop();
    }
    if body.is_empty() {
        return kind.as_str().to_string();
    }
    format!("{}-{}", kind.as_str(), body)
}

/// Confidence in `0.0..=1.0` from the evidence behind a lesson group: more
/// distinct sessions and verification raise it, on a saturating curve so a
/// single very-repeated lesson cannot reach certainty on count alone.
///
/// `1 - 1/(1 + distinct)` gives 0.5 at one session, 0.66 at two, 0.75 at
/// three…; verification adds a flat 0.25 bonus, clamped to 1.0.
#[must_use]
fn confidence_from_evidence(distinct_sessions: usize, verified: bool) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "the confidence calculation and its return value are f32"
    )]
    let base = 1.0 - 1.0 / (1.0 + distinct_sessions as f32);
    let bonus = if verified { 0.25 } else { 0.0 };
    (base + bonus).min(1.0)
}

/// A minimal, IO-free digest of one externalized turn — the cross-session
/// friction signal the Dreamer mines, deliberately decoupled from the runtime's
/// `TurnRecord` type so this brain stays portable and dependency-free.
///
/// Only the fields that can seed a *durable* lesson are kept: which session the
/// turn belonged to (for cross-session repetition counting) and which distinct
/// tools errored in it (the attributable friction). Counts, tokens, and goals
/// are audit-trail detail the curation policy does not act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnDigest {
    /// The session this turn belonged to; distinct values across a tool's
    /// failures are what satisfy [`PromotionPolicy::min_distinct_sessions`].
    pub session_id: String,
    /// Distinct tool names that produced an error this turn, in first-seen
    /// order. Empty for a clean turn (which yields no lesson).
    pub error_tools: Vec<String>,
}

/// Distill recurring-tool-failure lessons from externalized turn digests (pure).
///
/// Each `(session, errored-tool)` pair becomes one [`LessonKind::Gotcha`]
/// observation keyed on the tool name, so the *same* tool failing in distinct
/// sessions dedups into one group and — once it clears the promotion gate's
/// [`min_distinct_sessions`](PromotionPolicy::min_distinct_sessions) bar —
/// becomes a durable "this tool is error-prone here, check its preconditions"
/// lesson (doc §12-2: a coding agent remembers recurring failures).
///
/// The observations are marked `verified: true` because they are grounded in
/// real recorded `is_error` tool results — an objective event, not a model
/// inference — exactly the honesty bar [`verified_check_observation`] uses. The
/// per-run promotion budget and the cross-session repetition requirement remain
/// the only brakes, so a single transient error never pollutes memory: it must
/// recur in [`min_distinct_sessions`](PromotionPolicy::min_distinct_sessions)
/// distinct sessions first.
///
/// Determinism: within a session the same tool collapses to one observation
/// (first-seen order preserved), so re-running over the same digests yields the
/// same observation list — a property the unit tests pin.
///
/// [`verified_check_observation`]: crate::dreamer
#[must_use]
pub fn lessons_from_turns(digests: &[TurnDigest]) -> Vec<LessonObservation> {
    let mut out = Vec::new();
    for digest in digests {
        // Collapse repeats of the same tool within one turn/session: the gate
        // counts *distinct sessions*, so a duplicate (session, tool) observation
        // adds no signal and would only bloat the candidate pool.
        let mut seen_in_session: std::collections::BTreeSet<&str> =
            std::collections::BTreeSet::new();
        for tool in &digest.error_tools {
            let tool = tool.trim();
            if tool.is_empty() || !seen_in_session.insert(tool) {
                continue;
            }
            out.push(LessonObservation {
                // Keyed on the tool alone so the same failing tool dedups across
                // sessions regardless of which task triggered it.
                signature: format!("recurring tool failure: {tool}"),
                session_id: digest.session_id.clone(),
                lesson: format!(
                    "`{tool}` has errored across multiple sessions in this project. \
                     Before relying on it, double-check its inputs and preconditions \
                     (paths, arguments, required state) — its failures recur rather \
                     than being one-off."
                ),
                summary: format!(
                    "`{tool}` is error-prone here — verify its inputs before relying on it"
                ),
                kind: LessonKind::Gotcha,
                // Grounded in real recorded `is_error` results, not a model
                // claim — verified in the same sense as a green check command.
                verified: true,
            });
        }
    }
    out
}

/// A minimal, IO-free digest of one goal/loop automation event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationDigest {
    /// The session this event belonged to; distinct values satisfy the same
    /// cross-session repetition gate as turn digests.
    pub session_id: String,
    /// Automation family, currently `goal` or `loop`.
    pub kind: String,
    /// Coarse event name such as `succeeded`, `failed`, `unverified`,
    /// `repair_queued`, `fired`, or `stopped`.
    pub event: String,
    /// Whether the event is grounded in an objective controller outcome.
    pub verified: bool,
}

fn automation_lesson_template(
    kind: &str,
    event: &str,
) -> Option<(&'static str, LessonKind, &'static str, &'static str)> {
    match (kind, event) {
        ("goal", "succeeded") => Some((
            "goal automation reached validated success",
            LessonKind::Workflow,
            "Goal automation can complete after its validation gate succeeds",
            "`/goal` automation has repeatedly reached a validated success state in this project. Keep goal runs tied to their validator/semantic gate and preserve the bounded repair loop that lets the controller converge.",
        )),
        ("goal", "failed") => Some((
            "goal automation hit the turn cap",
            LessonKind::Gotcha,
            "Goal automation can hit its turn cap before validation succeeds",
            "`/goal` automation has repeatedly exhausted its turn budget before satisfying validation. Prefer smaller goal slices or stronger early verification so repair turns do not burn through the cap.",
        )),
        ("goal", "unverified") => Some((
            "goal automation ended unverified",
            LessonKind::Gotcha,
            "Goal automation can end unverified without a validator result",
            "`/goal` automation has repeatedly ended without a validator-backed success signal. Attach an explicit check command or semantic validator before trusting completion.",
        )),
        ("goal", "repair_queued") => Some((
            "goal automation queued repair",
            LessonKind::Gotcha,
            "Goal automation often needs a repair turn before completion",
            "`/goal` automation has repeatedly queued repair prompts. Keep repair prompts specific, preserve the latest validation failure, and avoid declaring success until the next validation gate passes.",
        )),
        ("loop", "fired") => Some((
            "loop automation fired scheduled prompt",
            LessonKind::Workflow,
            "Loop automation repeatedly fires scheduled prompts through the plan-first path",
            "`/loop` automation repeatedly fires scheduled prompts in this project. Preserve the plan-first marker and keep each scheduled run bounded so background loops do not drift from the user's objective.",
        )),
        ("loop", "stopped") => Some((
            "loop automation stopped",
            LessonKind::Workflow,
            "Loop automation can stop cleanly after its configured run boundary",
            "`/loop` automation has repeatedly stopped at its configured boundary. Keep fixed-count/interval loops explicit about remaining runs and stop reasons so users can trust long-running automation state.",
        )),
        _ => None,
    }
}

/// Distill goal/loop automation events into conservative Dreamer observations.
#[must_use]
pub fn lessons_from_automation(digests: &[AutomationDigest]) -> Vec<LessonObservation> {
    #[derive(Default)]
    struct SessionAutomationState {
        saw_goal_repair: bool,
        saw_repair_then_verified_goal_success: bool,
    }

    let mut out = Vec::new();
    let mut session_state: BTreeMap<String, SessionAutomationState> = BTreeMap::new();
    let mut seen: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();

    for digest in digests {
        let kind = digest.kind.trim();
        let event = digest.event.trim();
        if kind.is_empty() || event.is_empty() || digest.session_id.trim().is_empty() {
            continue;
        }
        let state = session_state.entry(digest.session_id.clone()).or_default();
        match (kind, event) {
            ("goal", "repair_queued") => state.saw_goal_repair = true,
            ("goal", "succeeded") if digest.verified && state.saw_goal_repair => {
                state.saw_repair_then_verified_goal_success = true;
            }
            _ => {}
        }

        let Some((signature, lesson_kind, summary, lesson)) =
            automation_lesson_template(kind, event)
        else {
            continue;
        };
        let key = (digest.session_id.clone(), signature.to_string());
        if !seen.insert(key) {
            continue;
        }
        out.push(LessonObservation {
            signature: signature.to_string(),
            session_id: digest.session_id.clone(),
            lesson: lesson.to_string(),
            summary: summary.to_string(),
            kind: lesson_kind,
            // Raw automation events are controller facts, not objective
            // verification evidence. Keep them as curation context only; the
            // promotion gate will skip them unless a verified pattern below
            // upgrades the signal.
            verified: false,
        });
    }

    for (session_id, state) in session_state {
        if !state.saw_repair_then_verified_goal_success {
            continue;
        }
        let signature = "goal automation repaired then succeeded";
        let key = (session_id.clone(), signature.to_string());
        if !seen.insert(key) {
            continue;
        }
        out.push(LessonObservation {
            signature: signature.to_string(),
            session_id,
            lesson: "`/goal` automation has repeatedly recovered from a repair prompt and then reached a validator-backed success. Preserve the repair→verify→accept loop: keep the failure details in the repair prompt and do not mark the goal done until the next verification gate succeeds.".to_string(),
            summary: "Goal automation can repair a failed turn and then succeed under validation".to_string(),
            kind: LessonKind::Workflow,
            verified: true,
        });
    }

    out
}

/// The curation gate: group observations by signature, then promote only the
/// groups that clear every brake in `policy`, deterministically and within
/// budget. `existing_slugs` are slugs already in long-term memory, so a re-run
/// is idempotent (already-known signatures are skipped, not rewritten).
///
/// Determinism: grouping is by `BTreeMap`, and the final order is confidence
/// descending then slug ascending, so the same inputs always yield the same
/// plan — a property the unit tests pin.
#[must_use]
pub fn curate(
    observations: &[LessonObservation],
    existing_slugs: &[String],
    policy: PromotionPolicy,
) -> CurationPlan {
    let existing = existing_slugs
        .iter()
        .cloned()
        .map(|slug| (slug, String::new()))
        .collect::<Vec<_>>();
    curate_with_existing_summaries(observations, &existing, policy)
}

/// Apply [`curate`] with the summaries already present in long-term memory.
/// Summary templates seed the per-pass representatives, so a new signature
/// cannot grow a near-duplicate family created by an earlier curation pass.
#[must_use]
pub fn curate_with_existing_summaries(
    observations: &[LessonObservation],
    existing: &[(String, String)],
    policy: PromotionPolicy,
) -> CurationPlan {
    // 1. Group observations by signature. BTreeMap keeps the pass deterministic
    //    and groups cross-session repetitions of the same lesson together.
    struct Group<'a> {
        kind: LessonKind,
        summary: &'a str,
        lesson: &'a str,
        sessions: std::collections::BTreeSet<&'a str>,
        verified: bool,
    }
    let mut groups: BTreeMap<&str, Group<'_>> = BTreeMap::new();
    for obs in observations {
        let group = groups.entry(obs.signature.as_str()).or_insert(Group {
            kind: obs.kind,
            summary: obs.summary.as_str(),
            lesson: obs.lesson.as_str(),
            sessions: std::collections::BTreeSet::new(),
            verified: false,
        });
        group.sessions.insert(obs.session_id.as_str());
        group.verified |= obs.verified;
    }

    let known: std::collections::BTreeSet<&str> =
        existing.iter().map(|(slug, _)| slug.as_str()).collect();
    let min_sessions = policy.min_distinct_sessions.max(1);

    // 2. Evaluate every group against the gate, collecting candidates and
    //    skip reasons. We rank approved candidates before applying the budget
    //    so the *best* lessons win a scarce budget, not just the first seen.
    let mut candidates: Vec<PromotedLesson> = Vec::new();
    let mut skipped: Vec<SkippedLesson> = Vec::new();

    for (signature, group) in &groups {
        let slug = slug_for(group.kind, signature);
        let distinct = group.sessions.len();
        let push_skip = |skipped: &mut Vec<SkippedLesson>, reason: SkipReason| {
            skipped.push(SkippedLesson {
                slug: slug.clone(),
                summary: group.summary.to_string(),
                reason,
            });
        };

        if known.contains(slug.as_str()) {
            push_skip(&mut skipped, SkipReason::AlreadyKnown);
            continue;
        }
        if distinct < min_sessions {
            push_skip(
                &mut skipped,
                SkipReason::NotRepeatedEnough {
                    distinct_sessions: distinct,
                    required: min_sessions,
                },
            );
            continue;
        }
        if policy.require_verified && !group.verified {
            push_skip(&mut skipped, SkipReason::Unverified);
            continue;
        }
        let confidence = confidence_from_evidence(distinct, group.verified);
        if confidence < policy.confidence_floor {
            push_skip(
                &mut skipped,
                SkipReason::BelowConfidenceFloor {
                    confidence,
                    floor: policy.confidence_floor,
                },
            );
            continue;
        }
        candidates.push(PromotedLesson {
            slug,
            summary: group.summary.to_string(),
            lesson: group.lesson.to_string(),
            kind: group.kind,
            distinct_sessions: distinct,
            verified: group.verified,
            confidence,
            expiry_days: policy.expiry_days,
        });
    }

    // 3. Rank by confidence desc, then slug asc for a stable tie-break.
    candidates.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.slug.cmp(&b.slug))
    });

    // 4. Coalesce approved lessons that differ only in inline-code values in
    // their summary template. Existing summaries seed the representatives;
    // among new candidates, the strongest one is already first.
    let mut candidates = coalesce_summary_templates(candidates, existing, &mut skipped);

    // 5. Apply the per-run budget: everything past the cap is skipped with an
    //    explicit BudgetExhausted reason (auditable, not silently dropped).
    let promote = if policy.max_promotions_per_run == 0
        || candidates.len() <= policy.max_promotions_per_run
    {
        candidates
    } else {
        let overflow = candidates.split_off(policy.max_promotions_per_run);
        for lesson in overflow {
            skipped.push(SkippedLesson {
                slug: lesson.slug,
                summary: lesson.summary,
                reason: SkipReason::BudgetExhausted,
            });
        }
        candidates
    };

    CurationPlan { promote, skipped }
}

fn coalesce_summary_templates(
    candidates: Vec<PromotedLesson>,
    existing: &[(String, String)],
    skipped: &mut Vec<SkippedLesson>,
) -> Vec<PromotedLesson> {
    let mut representatives: BTreeMap<String, String> = BTreeMap::new();
    for (slug, summary) in existing {
        if summary.is_empty() {
            continue;
        }
        representatives
            .entry(normalized_summary_template(summary))
            .and_modify(|representative| {
                if slug < representative {
                    representative.clone_from(slug);
                }
            })
            .or_insert_with(|| slug.clone());
    }

    let mut coalesced = Vec::with_capacity(candidates.len());
    for lesson in candidates {
        let family = normalized_summary_template(&lesson.summary);
        if let Some(promoted_slug) = representatives.get(&family) {
            skipped.push(SkippedLesson {
                slug: lesson.slug,
                summary: lesson.summary,
                reason: SkipReason::SameSummaryTemplate {
                    promoted_slug: promoted_slug.clone(),
                },
            });
        } else {
            representatives.insert(family, lesson.slug.clone());
            coalesced.push(lesson);
        }
    }
    coalesced
}

fn normalized_summary_template(summary: &str) -> String {
    let mut normalized = String::with_capacity(summary.len());
    let mut in_inline_code = false;
    for ch in summary.chars() {
        if ch == '`' {
            normalized.push(ch);
            if in_inline_code {
                in_inline_code = false;
            } else {
                normalized.push_str("{value}");
                in_inline_code = true;
            }
        } else if !in_inline_code {
            normalized.push(ch);
        }
    }
    if in_inline_code {
        summary.to_string()
    } else {
        normalized
    }
}

// ---------------------------------------------------------------------------
// Typed relations: how one promoted lesson stands to a page already written.
// ---------------------------------------------------------------------------

/// How a promoted lesson stands to a page already in the vault.
///
/// The spellings are the vault's own frontmatter keys, shared with
/// `runtime::second_brain::corpus::RelationKind` and with the window's graph
/// view. They are a contract, not a display string — a graph that reads
/// `depends_on` and a writer that emits `dependsOn` fail silently: the edge is
/// simply never drawn. `runtime::second_brain::promote` pins the equality in a
/// test, which is the only place both spellings are in scope at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LessonRelation {
    /// The lesson's evidence cites the page — a wikilink, or a path its slug
    /// answers to.
    DependsOn,
    /// The lesson restates a page this curation pass replaced.
    Supersedes,
    /// Verified evidence says the opposite of what the page claims.
    Contradicts,
}

impl LessonRelation {
    /// The frontmatter key and the verb in the vault's ingest log.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DependsOn => "depends_on",
            Self::Supersedes => "supersedes",
            Self::Contradicts => "contradicts",
        }
    }

    /// Order the tests are applied in when more than one could fit, strongest
    /// first. A lesson that both cites a page and refutes it is a
    /// contradiction — the citation is only how it found the claim.
    pub const RANKED: [Self; 3] = [Self::Contradicts, Self::Supersedes, Self::DependsOn];
}

/// The frontmatter key for the untyped fallback, named here so a caller that
/// files the leftovers cannot spell it differently from the typed three.
pub const RELATED_RELATION_KEY: &str = "related";

/// What a claim says about whether something holds.
///
/// Deliberately three-valued and deliberately small. This is a *polarity*
/// judgement, not sentiment: the only question is whether a sentence asserts
/// that something works or asserts that it does not, and everything that does
/// neither clearly is [`Polarity::Unknown`] — which contradicts nothing. The
/// asymmetry is on purpose. A missed contradiction costs one absent edge; a
/// false one writes a permanent, never-rewritten claim into a vault a person
/// edits by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Polarity {
    /// "`cargo test` passes", "the gate ran green", "합격".
    Affirms,
    /// "`cargo test` fails", "the gate is broken", "실패".
    Denies,
    /// No marker, or markers pointing both ways.
    Unknown,
}

impl Polarity {
    const fn flip(self) -> Self {
        match self {
            Self::Affirms => Self::Denies,
            Self::Denies => Self::Affirms,
            Self::Unknown => Self::Unknown,
        }
    }
}

/// Whole words that assert a thing holds. Matched by equality: `contains`
/// would let "node" carry the negator "no" and "working" is already listed.
const AFFIRMING: [&str; 10] = [
    "passes",
    "passed",
    "green",
    "succeeds",
    "succeeded",
    "works",
    "worked",
    "working",
    "fixed",
    "supported",
];

/// Whole words that assert a thing does not hold.
const DENYING: [&str; 12] = [
    "fails",
    "failed",
    "failing",
    "failure",
    "red",
    "broken",
    "breaks",
    "error",
    "errors",
    "unsupported",
    "rejects",
    "rejected",
];

/// Korean carries its markers inside inflected words, so these three are
/// matched as substrings. Each is specific enough that a false hit needs a
/// deliberately odd word.
const AFFIRMING_INFIX: [&str; 4] = ["성공", "초록", "합격", "통과"];
const DENYING_INFIX: [&str; 5] = ["실패", "빨강", "불가", "거부", "오류"];

/// English negators, which flip the marker they precede.
const NEGATORS: [&str; 5] = ["not", "no", "never", "cannot", "without"];

/// Korean negators, which are *post-positional*: they flip the marker that came
/// before them ("성공하지 못했다" is a denial of "성공"). Matched as substrings
/// for the same reason as the marker infixes. `안` is absent on purpose — it is
/// a syllable in dozens of ordinary words ("안전", "제안"), so it is only read
/// as a negator when it stands alone, via [`NEGATORS_KO_WORD`].
const NEGATOR_INFIX: [&str; 3] = ["않", "없", "못"];

/// Korean negators that only count as whole words.
const NEGATORS_KO_WORD: [&str; 2] = ["안", "아니"];

/// How many words an English negator reaches forward over. Three covers
/// "does not currently pass" without letting one negator colour a whole
/// paragraph.
const NEGATION_WINDOW: usize = 3;

/// The polarity a single word carries on its own, if any. Denial wins when a
/// word matches both, because the denying spellings are the more specific ones.
fn word_polarity(word: &str) -> Option<Polarity> {
    if DENYING.contains(&word) || DENYING_INFIX.iter().any(|mark| word.contains(mark)) {
        return Some(Polarity::Denies);
    }
    if AFFIRMING.contains(&word) || AFFIRMING_INFIX.iter().any(|mark| word.contains(mark)) {
        return Some(Polarity::Affirms);
    }
    None
}

/// Read the polarity of one claim.
///
/// One pass over the words. An English negator arms a forward flip for the next
/// `NEGATION_WINDOW` words; a Korean negator flips the marker already read,
/// or — when there is none to flip — denies on its own ("증거가 없다"). A claim
/// carrying markers both ways is [`Polarity::Unknown`]: "the old flag fails,
/// the new one passes" is a migration note, and reading it as a denial would
/// let it contradict every page saying the new flag works.
#[must_use]
pub fn claim_polarity(claim: &str) -> Polarity {
    let mut read: Vec<Polarity> = Vec::new();
    let mut armed = 0usize;
    for word in claim_words(claim) {
        if let Some(polarity) = word_polarity(&word) {
            read.push(if armed > 0 { polarity.flip() } else { polarity });
        }
        if NEGATOR_INFIX.iter().any(|mark| word.contains(mark))
            || NEGATORS_KO_WORD.contains(&word.as_str())
        {
            match read.last_mut() {
                Some(last) => *last = last.flip(),
                None => read.push(Polarity::Denies),
            }
        }
        armed = if NEGATORS.contains(&word.as_str()) {
            NEGATION_WINDOW
        } else {
            armed.saturating_sub(1)
        };
    }
    let affirms = read.iter().filter(|p| **p == Polarity::Affirms).count();
    let denies = read.iter().filter(|p| **p == Polarity::Denies).count();
    match (affirms, denies) {
        (0, 0) => Polarity::Unknown,
        (0, _) => Polarity::Denies,
        (_, 0) => Polarity::Affirms,
        _ => Polarity::Unknown,
    }
}

/// Lowercased words of a claim, punctuation and markup dropped. Korean survives
/// as whole space-separated words, which is why the infix lists exist.
fn claim_words(claim: &str) -> impl Iterator<Item = String> + '_ {
    claim
        .split(|ch: char| !ch.is_alphanumeric() && !matches!(ch, '_' | '-'))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

/// One page already in the vault, as much of it as a shutdown-time judgement
/// can afford to know: its slug and its one-line claim. No page body is opened
/// to build one of these — see `promote::related_candidates`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultPage {
    /// Vault-relative slug, e.g. `wiki/zo/gotcha-sqlite-busy`.
    pub slug: String,
    /// The page's pointer summary — the claim this judgement reads.
    pub claim: String,
}

/// A promoted lesson, reduced to what the relation judgement reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LessonEvidence {
    /// The lesson's own memory slug (not vault-relative).
    pub slug: String,
    /// The lesson's claim — its summary and body, joined by the caller.
    pub claim: String,
    /// Targets the evidence cites, from [`cited_targets`].
    pub cites: Vec<String>,
    /// Slugs this curation pass judged the lesson replaces: the entries whose
    /// index summary it swaps out.
    pub replaces: Vec<String>,
    /// Whether the lesson rests on an objective check. Only verified evidence
    /// may contradict a page — a model inference disagreeing with something a
    /// person wrote is not a finding.
    pub verified: bool,
}

/// Targets a lesson's evidence cites: `[[wikilink]]` bodies first, then
/// path-shaped tokens. Deduped, in first-seen order, so two runs over one
/// lesson cite the same pages in the same order.
#[must_use]
pub fn cited_targets(evidence: &str) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    let mut push = |value: &str| {
        let value = value.trim().trim_matches('/');
        if !value.is_empty() && !targets.iter().any(|existing| existing == value) {
            targets.push(value.to_string());
        }
    };
    // Wikilinks first, and the text they occupied is dropped from what the
    // path scan then reads — otherwise `[[wiki/a|label]]` is cited twice, once
    // as its target and once as the path-shaped word `wiki/a|label`.
    let mut rest = evidence;
    let mut prose = String::with_capacity(evidence.len());
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else { break };
        // An alias (`[[slug|label]]`) points at the slug, never at the label.
        push(after[..close].split('|').next().unwrap_or_default());
        prose.push_str(&rest[..open]);
        prose.push(' ');
        rest = &after[close + 2..];
    }
    prose.push_str(rest);
    for word in prose.split_whitespace() {
        let word = word.trim_matches(|ch: char| {
            !ch.is_alphanumeric() && !matches!(ch, '/' | '.' | '_' | '-')
        });
        // A URL is a citation of the internet, not of this vault.
        if word.contains('/') && !word.contains("://") {
            push(word);
        }
    }
    targets
}

/// Tokens two claims must share before their polarities are compared at all.
/// One shared word is a coincidence, and comparing polarity across unrelated
/// subjects is how "the build fails" ends up contradicting "the parser works".
const MIN_SUBJECT_OVERLAP: usize = 2;

/// Judge how `lesson` stands to `page`, or `None` when no typed relation
/// applies and the page is left to the caller's untyped overlap fallback.
///
/// Pure and total: the same pair always yields the same verdict, and no branch
/// reads a clock, a file, or a config. The tests are applied in
/// [`LessonRelation::RANKED`] order.
///
/// - **Contradicts** — the lesson is verified, the two claims are about the
///   same subject (`MIN_SUBJECT_OVERLAP` shared words), and their polarities
///   are opposite. [`Polarity::Unknown`] on either side means there is nothing
///   to compare.
/// - **Supersedes** — the curation pass named this page among the ones the
///   lesson replaces, or the page's slug is the lesson's slug with segments
///   trimmed, which is what a re-promotion under a refined signature looks like.
/// - **`DependsOn`** — the evidence cites the page.
#[must_use]
pub fn judge_relation(lesson: &LessonEvidence, page: &VaultPage) -> Option<LessonRelation> {
    LessonRelation::RANKED
        .into_iter()
        .find(|relation| match relation {
            LessonRelation::Contradicts => contradicts(lesson, page),
            LessonRelation::Supersedes => supersedes(lesson, page),
            LessonRelation::DependsOn => cites(lesson, page),
        })
}

fn contradicts(lesson: &LessonEvidence, page: &VaultPage) -> bool {
    if !lesson.verified || subject_overlap(&lesson.claim, &page.claim) < MIN_SUBJECT_OVERLAP {
        return false;
    }
    matches!(
        (claim_polarity(&lesson.claim), claim_polarity(&page.claim)),
        (Polarity::Affirms, Polarity::Denies) | (Polarity::Denies, Polarity::Affirms)
    )
}

fn supersedes(lesson: &LessonEvidence, page: &VaultPage) -> bool {
    let leaf = slug_leaf(&page.slug);
    if lesson
        .replaces
        .iter()
        .any(|replaced| slug_leaf(replaced) == leaf)
    {
        return true;
    }
    // A refined signature promotes under a longer slug in the same family:
    // `workflow-verified-check-cargo-test` refines `workflow-verified-check`.
    // Segment-wise, so `gotcha-read` never claims to supersede `gotcha-readme`.
    let mine: Vec<&str> = lesson.slug.split('-').collect();
    let theirs: Vec<&str> = leaf.split('-').collect();
    theirs.len() >= 2 && theirs.len() < mine.len() && mine.starts_with(theirs.as_slice())
}

fn cites(lesson: &LessonEvidence, page: &VaultPage) -> bool {
    lesson.cites.iter().any(|target| {
        let target = target.trim_end_matches(".md");
        target == page.slug || slug_leaf(target) == slug_leaf(&page.slug)
    })
}

/// The last path segment of a slug — `wiki/zo/gotcha-x` and `gotcha-x` are one
/// lesson filed at two depths.
fn slug_leaf(slug: &str) -> &str {
    slug.rsplit('/').next().unwrap_or(slug)
}

/// How many subject words two claims share. Polarity markers and negators are
/// excluded: two sentences agreeing only on the word "fails" are not about the
/// same subject, and counting them would make every failure note contradict
/// every success note.
fn subject_overlap(left: &str, right: &str) -> usize {
    let left: BTreeSet<String> = subject_words(left).collect();
    let right: BTreeSet<String> = subject_words(right).collect();
    left.intersection(&right).count()
}

fn subject_words(claim: &str) -> impl Iterator<Item = String> + '_ {
    claim_words(claim).filter(|word| {
        word.chars().count() >= 2 && word_polarity(word).is_none() && !is_negator(word)
    })
}

fn is_negator(word: &str) -> bool {
    NEGATORS.contains(&word)
        || NEGATORS_KO_WORD.contains(&word)
        || NEGATOR_INFIX.iter().any(|mark| word.contains(mark))
}

#[cfg(test)]
mod tests {
    use super::{
        AdvisorFinding, AdvisorRole, ApplyGateInput, AutomationDigest, CandidateEvidence,
        CandidateKind, CandidateStatus, DreamJudgeDecision, LessonKind, LessonObservation,
        PatchRisk, PromotionPolicy, SelfImproveCandidate, SkipReason, TurnDigest, candidate_score,
        candidate_score_at, confidence_from_evidence, curate, curate_with_existing_summaries,
        decide_apply_gate, lessons_from_automation, lessons_from_turns,
        rank_self_improve_candidates, rank_self_improve_candidates_at,
        representative_candidate_evidence, self_improve_candidate_id, slug_for,
        synthesize_dream_fusion, cited_targets, claim_polarity, judge_relation, LessonEvidence,
        LessonRelation, Polarity, VaultPage, RELATED_RELATION_KEY,
    };

    fn obs(sig: &str, session: &str, verified: bool) -> LessonObservation {
        LessonObservation {
            signature: sig.to_string(),
            session_id: session.to_string(),
            lesson: format!("lesson body for {sig}"),
            summary: format!("summary for {sig}"),
            kind: LessonKind::Gotcha,
            verified,
        }
    }

    fn candidate_for_fusion() -> SelfImproveCandidate {
        SelfImproveCandidate::new(
            CandidateKind::TurnFailure,
            "turn failed after timeout",
            vec![CandidateEvidence {
                session_id: "s1".to_string(),
                source: "turn".to_string(),
                detail: "timeout".to_string(),
                verified: false,
            }],
        )
    }

    #[test]
    fn dream_fusion_synthesizes_read_only_advisor_findings() {
        let candidate = candidate_for_fusion();
        let findings = vec![
            AdvisorFinding {
                role: AdvisorRole::Risk,
                candidate_id: candidate.id.clone(),
                summary: "low blast radius".to_string(),
                confidence: 0.7,
                risk: PatchRisk::Low,
                recommended_checks: vec!["cargo test -p runtime dreamer --lib".to_string()],
                accepts_quarantine: true,
            },
            AdvisorFinding {
                role: AdvisorRole::RootCause,
                candidate_id: candidate.id.clone(),
                summary: "timeout handling regressed".to_string(),
                confidence: 0.8,
                risk: PatchRisk::Medium,
                recommended_checks: vec!["cargo test -p runtime dreamer --lib".to_string()],
                accepts_quarantine: true,
            },
        ];

        let report = synthesize_dream_fusion("run-1", &candidate, findings);

        assert_eq!(report.decision, DreamJudgeDecision::PlanPatch);
        assert_eq!(report.risk, PatchRisk::Medium);
        assert_eq!(
            report.required_checks,
            vec!["cargo test -p runtime dreamer --lib"]
        );
        assert_eq!(report.findings[0].role, AdvisorRole::RootCause);
        assert_eq!(report.findings[1].role, AdvisorRole::Risk);
    }

    #[test]
    fn dream_fusion_requires_evidence_or_quarantines_high_risk() {
        let candidate = candidate_for_fusion();
        let empty = synthesize_dream_fusion("run-empty", &candidate, Vec::new());
        assert_eq!(empty.decision, DreamJudgeDecision::NeedMoreEvidence);

        let high = synthesize_dream_fusion(
            "run-high",
            &candidate,
            vec![AdvisorFinding {
                role: AdvisorRole::Risk,
                candidate_id: candidate.id.clone(),
                summary: "touches permission core".to_string(),
                confidence: 0.9,
                risk: PatchRisk::High,
                recommended_checks: vec!["cargo test --workspace".to_string()],
                accepts_quarantine: true,
            }],
        );
        assert_eq!(high.decision, DreamJudgeDecision::Quarantine);
        assert_eq!(high.risk, PatchRisk::High);
    }

    #[test]
    fn apply_gate_requires_all_manual_conditions() {
        let accepted = decide_apply_gate(&ApplyGateInput {
            approved_by_user: true,
            clean_tree: true,
            base_commit_matches: true,
            paths_allowed: true,
            focused_checks_green: true,
            reviewer_accepted: true,
            risk: PatchRisk::Low,
        });
        assert!(accepted.eligible);
        assert!(accepted.reasons.is_empty());

        let rejected = decide_apply_gate(&ApplyGateInput {
            approved_by_user: false,
            clean_tree: false,
            base_commit_matches: false,
            paths_allowed: false,
            focused_checks_green: false,
            reviewer_accepted: false,
            risk: PatchRisk::High,
        });
        assert!(!rejected.eligible);
        assert_eq!(
            rejected.reasons,
            vec![
                "missing_user_approval",
                "worktree_not_clean",
                "base_commit_mismatch",
                "path_not_allowlisted",
                "focused_checks_not_green",
                "reviewer_not_accepted",
                "high_risk_patch",
            ]
        );
    }

    #[test]
    fn self_improve_candidate_ids_are_stable_and_sanitized() {
        assert_eq!(
            self_improve_candidate_id(CandidateKind::GoalTerminal, "Goal ended UNVERIFIED!!"),
            "goal_terminal-goal-ended-unverified"
        );
        assert_eq!(
            self_improve_candidate_id(CandidateKind::PostTurn, "   "),
            "post_turn"
        );
    }

    /// The signature label mines only bounded tokens — HTTP status and an
    /// `UPPER_SNAKE` provider code — so two unrelated provider failures land in
    /// distinct candidates while free text (paths, quotes, prose) never leaks
    /// into candidate identity.
    #[test]
    fn error_signature_label_extracts_status_and_code_only() {
        assert_eq!(
            super::error_signature_label(
                "provider_non_retryable",
                "api returned 400 Bad Request (INVALID_ARGUMENT): Invalid value at \
                 'metadata.platform' (type.googleapis.com/...Platform), \"MACOS\"",
            ),
            "provider_non_retryable · 400 INVALID_ARGUMENT"
        );
        assert_eq!(
            super::error_signature_label("provider_rate_limit", "api returned 429 too many requests"),
            "provider_rate_limit · 429"
        );
        assert_eq!(
            super::error_signature_label("provider_transient", "connection reset by peer"),
            "provider_transient"
        );
        assert_eq!(
            super::error_signature_label("", "stream ended unexpectedly"),
            "unclassified"
        );
        // A code without a status still segments, with a stable separator.
        assert_eq!(
            super::error_signature_label("runtime_error", "tool rejected: SCHEMA_MISMATCH on input"),
            "runtime_error · SCHEMA_MISMATCH"
        );
        // Over-long ALL-CAPS junk (e.g. a base64 blob) never becomes a key.
        let junk = "X".repeat(64);
        assert_eq!(
            super::error_signature_label("runtime_error", &junk),
            "runtime_error"
        );
    }

    /// Identity derivation: the label flows through `self_improve_candidate_id`
    /// into a per-signature id, replacing the single all-failures bucket.
    #[test]
    fn error_signature_label_yields_segmented_candidate_ids() {
        let label = super::error_signature_label(
            "provider_non_retryable",
            "api returned 400 Bad Request (INVALID_ARGUMENT)",
        );
        assert_eq!(
            self_improve_candidate_id(CandidateKind::TurnFailure, &format!("turn failure: {label}")),
            "turn_failure-turn-failure-provider-non-retryable-400-invalid-argument"
        );
    }

    #[test]
    fn self_improve_candidate_new_defaults_to_proposed() {
        let candidate = SelfImproveCandidate::new(
            CandidateKind::TurnFailure,
            "turn failed after timeout",
            vec![CandidateEvidence {
                session_id: "s1".to_string(),
                source: "turn".to_string(),
                detail: "timeout".to_string(),
                verified: false,
            }],
        );

        assert_eq!(candidate.id, "turn_failure-turn-failed-after-timeout");
        assert_eq!(candidate.status, CandidateStatus::Proposed);
        assert_eq!(candidate.evidence.len(), 1);
    }

    #[test]
    fn success_telemetry_is_terminal_and_not_scored_for_repair() {
        let post_turn = SelfImproveCandidate::new(
            CandidateKind::PostTurn,
            "successful turn persisted",
            vec![CandidateEvidence {
                session_id: "s1".to_string(),
                source: "post_turn".to_string(),
                detail: "persisted".to_string(),
                verified: true,
            }],
        );
        let verified_accept = SelfImproveCandidate::new(
            CandidateKind::VerifiedAccept,
            "check cargo test accepted",
            vec![CandidateEvidence {
                session_id: "s1".to_string(),
                source: "deep_gate".to_string(),
                detail: "cargo test".to_string(),
                verified: true,
            }],
        );
        let turn_failure = SelfImproveCandidate::new(
            CandidateKind::TurnFailure,
            "streaming turn failed",
            vec![CandidateEvidence {
                session_id: "s1".to_string(),
                source: "turn".to_string(),
                detail: "runtime error".to_string(),
                verified: false,
            }],
        );

        assert!(candidate_score(&turn_failure) > 0);
        assert_eq!(candidate_score(&verified_accept), 0);
        assert_eq!(candidate_score(&post_turn), 0);
        assert_eq!(verified_accept.status, CandidateStatus::Rejected);
        assert_eq!(post_turn.status, CandidateStatus::Rejected);
        let ranked = rank_self_improve_candidates(&[
            post_turn.clone(),
            turn_failure.clone(),
            verified_accept.clone(),
        ]);
        assert_eq!(ranked[0].id, turn_failure.id);
    }

    #[test]
    fn candidate_score_rewards_independent_sessions_not_repeated_noise() {
        let evidence = |session: &str| CandidateEvidence {
            session_id: session.to_string(),
            source: "turn".to_string(),
            detail: "same failure".to_string(),
            verified: false,
        };
        let one = SelfImproveCandidate::new(
            CandidateKind::TurnFailure,
            "failure",
            vec![evidence("s1")],
        );
        let repeated = SelfImproveCandidate::new(
            CandidateKind::TurnFailure,
            "failure",
            vec![evidence("s1"), evidence("s1"), evidence("s1")],
        );
        let independent = SelfImproveCandidate::new(
            CandidateKind::TurnFailure,
            "failure",
            vec![evidence("s1"), evidence("s2")],
        );

        assert_eq!(candidate_score(&one), candidate_score(&repeated));
        assert!(candidate_score(&independent) > candidate_score(&repeated));
    }

    #[test]
    fn cancellation_is_recordable_but_never_actionable() {
        let cancelled = SelfImproveCandidate::new(
            CandidateKind::UserCancelled,
            "turn cancelled by user or host",
            vec![CandidateEvidence {
                session_id: "s1".to_string(),
                source: "turn".to_string(),
                detail: "abort signal".to_string(),
                verified: true,
            }],
        );

        assert!(!cancelled.kind.is_actionable());
        assert_eq!(cancelled.status, CandidateStatus::Rejected);
        assert_eq!(candidate_score(&cancelled), 0);
        assert_eq!(candidate_score_at(&cancelled, 1), 0);
    }

    #[test]
    fn candidate_ranking_uses_explicit_recency_clock() {
        let mut old = SelfImproveCandidate::new(CandidateKind::TurnFailure, "old", Vec::new());
        old.last_observed_at_ms = 1;
        let mut recent = SelfImproveCandidate::new(CandidateKind::TurnFailure, "recent", Vec::new());
        recent.last_observed_at_ms = 40 * 24 * 60 * 60 * 1_000;
        let now = recent.last_observed_at_ms;

        assert!(candidate_score_at(&recent, now) > candidate_score_at(&old, now));
        let ranked = rank_self_improve_candidates_at(&[old, recent.clone()], now);
        assert_eq!(ranked[0].id, recent.id);
    }

    #[test]
    fn representative_evidence_prefers_verified_independent_sessions() {
        let candidate = SelfImproveCandidate::new(
            CandidateKind::TurnFailure,
            "failure",
            vec![
                CandidateEvidence {
                    session_id: "s1".to_string(),
                    source: "turn".to_string(),
                    detail: "unverified".to_string(),
                    verified: false,
                },
                CandidateEvidence {
                    session_id: "s1".to_string(),
                    source: "deep_gate".to_string(),
                    detail: "verified s1".to_string(),
                    verified: true,
                },
                CandidateEvidence {
                    session_id: "s2".to_string(),
                    source: "goal".to_string(),
                    detail: "verified s2".to_string(),
                    verified: true,
                },
            ],
        );

        let selected = representative_candidate_evidence(&candidate, 2);
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().all(|evidence| evidence.verified));
        assert_ne!(selected[0].session_id, selected[1].session_id);
    }

    #[test]
    fn representative_evidence_deduplicates_empty_sessions_by_source() {
        let candidate = SelfImproveCandidate::new(
            CandidateKind::TurnFailure,
            "host failure",
            vec![
                CandidateEvidence {
                    session_id: String::new(),
                    source: "host".to_string(),
                    detail: "older".to_string(),
                    verified: true,
                },
                CandidateEvidence {
                    session_id: String::new(),
                    source: "host".to_string(),
                    detail: "newer".to_string(),
                    verified: true,
                },
                CandidateEvidence {
                    session_id: String::new(),
                    source: "provider".to_string(),
                    detail: "provider evidence".to_string(),
                    verified: true,
                },
            ],
        );

        let selected = representative_candidate_evidence(&candidate, 3);
        assert_eq!(selected.len(), 2);
        assert_ne!(selected[0].source, selected[1].source);
    }

    #[test]
    fn slug_prefixes_kind_and_kebab_cases() {
        assert_eq!(
            slug_for(LessonKind::Gotcha, "SQLite BUSY timeout!!"),
            "gotcha-sqlite-busy-timeout"
        );
        assert_eq!(
            slug_for(LessonKind::Preference, "  spaces   collapse  "),
            "preference-spaces-collapse"
        );
    }

    #[test]
    fn promotes_only_repeated_and_verified_under_default_policy() {
        let policy = PromotionPolicy::default();
        let observations = vec![
            // Repeated across two sessions, verified → promote.
            obs("a", "s1", true),
            obs("a", "s2", false),
            // Seen twice but never verified → skipped (require_verified).
            obs("b", "s1", false),
            obs("b", "s2", false),
            // Verified but only one session → skipped (not repeated enough).
            obs("c", "s1", true),
        ];

        let plan = curate(&observations, &[], policy);

        let promoted: Vec<&str> = plan.promote.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(promoted, vec!["gotcha-a"]);
        assert_eq!(plan.promote[0].distinct_sessions, 2);
        assert!(plan.promote[0].verified);

        // Both rejects are present with their specific reasons.
        let reason_for = |slug: &str| {
            plan.skipped
                .iter()
                .find(|s| s.slug == slug)
                .map(|s| s.reason.clone())
        };
        assert_eq!(reason_for("gotcha-b"), Some(SkipReason::Unverified));
        assert!(matches!(
            reason_for("gotcha-c"),
            Some(SkipReason::NotRepeatedEnough {
                distinct_sessions: 1,
                required: 2
            })
        ));
    }

    #[test]
    fn existing_slugs_make_reruns_idempotent() {
        let policy = PromotionPolicy::permissive();
        let observations = vec![obs("a", "s1", true)];

        // First run would promote `gotcha-a`; if it already exists, skip it.
        let plan = curate(&observations, &["gotcha-a".to_string()], policy);

        assert!(plan.is_empty());
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, SkipReason::AlreadyKnown);
    }

    #[test]
    fn budget_caps_promotions_and_records_overflow() {
        let policy = PromotionPolicy {
            min_distinct_sessions: 1,
            require_verified: false,
            max_promotions_per_run: 1,
            confidence_floor: 0.0,
            expiry_days: 0,
        };
        // Two eligible lessons; `a` has more sessions → higher confidence → wins
        // the single budget slot; `b` overflows.
        let observations = vec![
            obs("a", "s1", true),
            obs("a", "s2", true),
            obs("b", "s1", false),
        ];

        let plan = curate(&observations, &[], policy);

        assert_eq!(plan.promote.len(), 1);
        assert_eq!(plan.promote[0].slug, "gotcha-a");
        assert_eq!(
            plan.skipped
                .iter()
                .find(|s| s.slug == "gotcha-b")
                .map(|s| s.reason.clone()),
            Some(SkipReason::BudgetExhausted)
        );
    }

    #[test]
    fn confidence_floor_rejects_weak_evidence() {
        let policy = PromotionPolicy {
            min_distinct_sessions: 1,
            require_verified: false,
            max_promotions_per_run: 0,
            confidence_floor: 0.6, // one unverified session = 0.5 < 0.6
            expiry_days: 0,
        };
        let plan = curate(&[obs("a", "s1", false)], &[], policy);

        assert!(plan.is_empty());
        assert!(matches!(
            plan.skipped[0].reason,
            SkipReason::BelowConfidenceFloor { .. }
        ));
    }

    #[test]
    fn confidence_curve_is_monotonic_and_bounded() {
        assert!((confidence_from_evidence(1, false) - 0.5).abs() < 1e-6);
        assert!(confidence_from_evidence(2, false) > confidence_from_evidence(1, false));
        assert!(confidence_from_evidence(100, true) <= 1.0);
        // Verification strictly helps at equal session counts.
        assert!(confidence_from_evidence(2, true) > confidence_from_evidence(2, false));
    }

    #[test]
    fn curation_is_deterministic() {
        let policy = PromotionPolicy::permissive();
        let observations = vec![
            obs("b", "s1", true),
            obs("a", "s1", true),
            obs("c", "s1", true),
        ];
        let first = curate(&observations, &[], policy);
        let second = curate(&observations, &[], policy);
        assert_eq!(first, second);
    }

    #[test]
    fn curation_coalesces_same_template_summaries_into_one_promotion() {
        let mut observations = Vec::new();
        for (tool, signature) in [
            ("bash", "recurring tool failure: bash"),
            ("read_file", "recurring tool failure: read_file"),
        ] {
            for session in ["s1", "s2"] {
                observations.push(LessonObservation {
                    signature: signature.to_string(),
                    session_id: session.to_string(),
                    lesson: format!("`{tool}` repeatedly failed; verify its inputs"),
                    summary: format!(
                        "`{tool}` is error-prone here — verify its inputs before relying on it"
                    ),
                    kind: LessonKind::Gotcha,
                    verified: true,
                });
            }
        }

        let plan = curate(&observations, &[], PromotionPolicy::default());

        assert_eq!(plan.promote.len(), 1, "curation plan: {plan:?}");
    }

    #[test]
    fn curation_does_not_grow_an_existing_summary_template_family() {
        let observations = ["s1", "s2"].map(|session| LessonObservation {
            signature: "recurring tool failure: read_file".to_string(),
            session_id: session.to_string(),
            lesson: "`read_file` repeatedly failed; verify its inputs".to_string(),
            summary: "`read_file` is error-prone here — verify its inputs before relying on it"
                .to_string(),
            kind: LessonKind::Gotcha,
            verified: true,
        });
        let existing = vec![(
            "gotcha-recurring-tool-failure-bash".to_string(),
            "`bash` is error-prone here — verify its inputs before relying on it".to_string(),
        )];

        let plan = curate_with_existing_summaries(
            &observations,
            &existing,
            PromotionPolicy::default(),
        );

        assert_eq!(
            (plan.promote.len(), plan.skipped[0].reason.clone()),
            (
                0,
                SkipReason::SameSummaryTemplate {
                    promoted_slug: "gotcha-recurring-tool-failure-bash".to_string(),
                },
            )
        );
    }

    #[test]
    fn turn_lessons_dedup_tools_within_a_session_and_key_on_the_tool() {
        let digests = vec![TurnDigest {
            session_id: "s1".to_string(),
            // `bash` errors twice in one session → one observation, not two.
            error_tools: vec![
                "bash".to_string(),
                "read_file".to_string(),
                "bash".to_string(),
            ],
        }];
        let lessons = lessons_from_turns(&digests);

        assert_eq!(lessons.len(), 2);
        // Both are verified Gotchas keyed on the tool name alone.
        for lesson in &lessons {
            assert!(lesson.verified);
            assert_eq!(lesson.kind, LessonKind::Gotcha);
            assert!(lesson.signature.starts_with("recurring tool failure: "));
        }
        let signatures: Vec<&str> = lessons.iter().map(|l| l.signature.as_str()).collect();
        assert!(signatures.contains(&"recurring tool failure: bash"));
        assert!(signatures.contains(&"recurring tool failure: read_file"));
    }

    #[test]
    fn turn_lessons_skip_clean_and_blank_tools() {
        let digests = vec![
            TurnDigest {
                session_id: "s1".to_string(),
                error_tools: Vec::new(), // a clean turn yields nothing
            },
            TurnDigest {
                session_id: "s2".to_string(),
                error_tools: vec!["   ".to_string()], // blank tool name ignored
            },
        ];
        assert!(lessons_from_turns(&digests).is_empty());
    }

    #[test]
    fn turn_lessons_feed_the_gate_and_need_distinct_sessions() {
        // The same tool failing in TWO distinct sessions promotes once; a single
        // session's failure does not (the cross-session repetition brake).
        let repeated = vec![
            TurnDigest {
                session_id: "s1".to_string(),
                error_tools: vec!["bash".to_string()],
            },
            TurnDigest {
                session_id: "s2".to_string(),
                error_tools: vec!["bash".to_string()],
            },
        ];
        let plan = curate(
            &lessons_from_turns(&repeated),
            &[],
            PromotionPolicy::default(),
        );
        let promoted: Vec<&str> = plan.promote.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(promoted, vec!["gotcha-recurring-tool-failure-bash"]);

        // One session alone → skipped as not-repeated-enough under the default gate.
        let single = vec![TurnDigest {
            session_id: "s1".to_string(),
            error_tools: vec!["bash".to_string()],
        }];
        assert!(
            curate(
                &lessons_from_turns(&single),
                &[],
                PromotionPolicy::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn turn_lessons_are_deterministic() {
        let digests = vec![
            TurnDigest {
                session_id: "s1".to_string(),
                error_tools: vec!["bash".to_string(), "grep".to_string()],
            },
            TurnDigest {
                session_id: "s2".to_string(),
                error_tools: vec!["grep".to_string()],
            },
        ];
        assert_eq!(lessons_from_turns(&digests), lessons_from_turns(&digests));
    }

    #[test]
    fn automation_repair_then_success_pattern_feeds_the_gate() {
        let observations = lessons_from_automation(&[
            AutomationDigest {
                session_id: "s1".to_string(),
                kind: "goal".to_string(),
                event: "repair_queued".to_string(),
                verified: false,
            },
            AutomationDigest {
                session_id: "s1".to_string(),
                kind: "goal".to_string(),
                event: "succeeded".to_string(),
                verified: true,
            },
            AutomationDigest {
                session_id: "s2".to_string(),
                kind: "goal".to_string(),
                event: "repair_queued".to_string(),
                verified: false,
            },
            AutomationDigest {
                session_id: "s2".to_string(),
                kind: "goal".to_string(),
                event: "succeeded".to_string(),
                verified: true,
            },
            AutomationDigest {
                session_id: "s1".to_string(),
                kind: "goal".to_string(),
                event: "failed".to_string(),
                verified: true,
            },
        ]);

        assert!(
            observations
                .iter()
                .all(|obs| !obs.lesson.contains("secret goal text"))
        );
        assert!(observations.iter().any(|obs| {
            obs.signature == "goal automation repaired then succeeded" && obs.verified
        }));
        assert!(
            observations
                .iter()
                .filter(|obs| obs.signature == "goal automation hit the turn cap")
                .all(|obs| !obs.verified)
        );

        let plan = curate(&observations, &[], PromotionPolicy::default());
        let promoted: Vec<&str> = plan.promote.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(
            promoted,
            vec!["workflow-goal-automation-repaired-then-succeeded"]
        );
    }

    #[test]
    fn automation_success_before_repair_does_not_promote_repair_pattern() {
        let observations = lessons_from_automation(&[
            AutomationDigest {
                session_id: "s1".to_string(),
                kind: "goal".to_string(),
                event: "succeeded".to_string(),
                verified: true,
            },
            AutomationDigest {
                session_id: "s1".to_string(),
                kind: "goal".to_string(),
                event: "repair_queued".to_string(),
                verified: false,
            },
            AutomationDigest {
                session_id: "s2".to_string(),
                kind: "goal".to_string(),
                event: "succeeded".to_string(),
                verified: true,
            },
            AutomationDigest {
                session_id: "s2".to_string(),
                kind: "goal".to_string(),
                event: "repair_queued".to_string(),
                verified: false,
            },
        ]);

        assert!(
            !observations
                .iter()
                .any(|obs| obs.signature == "goal automation repaired then succeeded")
        );
        assert!(curate(&observations, &[], PromotionPolicy::default()).is_empty());
    }

    #[test]
    fn automation_lessons_skip_unknown_events_and_dedup_one_session() {
        let observations = lessons_from_automation(&[
            AutomationDigest {
                session_id: "s1".to_string(),
                kind: "goal".to_string(),
                event: "repair_queued".to_string(),
                verified: false,
            },
            AutomationDigest {
                session_id: "s1".to_string(),
                kind: "goal".to_string(),
                event: "repair_queued".to_string(),
                verified: false,
            },
            AutomationDigest {
                session_id: "s1".to_string(),
                kind: "unknown".to_string(),
                event: "failed".to_string(),
                verified: true,
            },
        ]);

        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].signature, "goal automation queued repair");
        assert!(!observations[0].verified);
    }

    // -- typed relations (§1.1): the pure polarity judgement -----------------

    fn page(slug: &str, claim: &str) -> VaultPage {
        VaultPage {
            slug: slug.to_string(),
            claim: claim.to_string(),
        }
    }

    fn verified_lesson(slug: &str, claim: &str) -> LessonEvidence {
        LessonEvidence {
            slug: slug.to_string(),
            claim: claim.to_string(),
            verified: true,
            ..LessonEvidence::default()
        }
    }

    #[test]
    fn polarity_reads_plain_affirmations_and_denials() {
        assert_eq!(claim_polarity("`cargo test` passes here"), Polarity::Affirms);
        assert_eq!(claim_polarity("`cargo test` fails here"), Polarity::Denies);
        assert_eq!(
            claim_polarity("the pane is opened with a pty"),
            Polarity::Unknown,
            "a sentence with no marker must not be given one"
        );
    }

    #[test]
    fn polarity_flips_behind_a_negator_and_only_inside_its_window() {
        assert_eq!(
            claim_polarity("`cargo test` does not pass"),
            Polarity::Unknown,
            "`pass` is not a listed marker; only `passes` is"
        );
        assert_eq!(
            claim_polarity("`cargo test` does not currently succeeded"),
            Polarity::Denies
        );
        assert_eq!(
            claim_polarity("never green"),
            Polarity::Denies,
            "a negator flips the marker it precedes"
        );
        assert_eq!(
            claim_polarity("no cache, one two three four, the gate passes"),
            Polarity::Affirms,
            "the window must not colour a whole sentence"
        );
    }

    #[test]
    fn polarity_reads_korean_post_positional_negation() {
        assert_eq!(claim_polarity("게이트가 통과했다"), Polarity::Affirms);
        assert_eq!(claim_polarity("게이트가 실패했다"), Polarity::Denies);
        assert_eq!(
            claim_polarity("게이트가 성공하지 못했다"),
            Polarity::Denies,
            "못 flips the marker before it, not after"
        );
        assert_eq!(
            claim_polarity("게이트가 실패하지 않는다"),
            Polarity::Affirms,
            "않 flips a denial into an affirmation"
        );
        assert_eq!(
            claim_polarity("재현 증거가 없다"),
            Polarity::Denies,
            "a bare Korean negator denies on its own"
        );
        assert_eq!(
            claim_polarity("제안한 안전 장치"),
            Polarity::Unknown,
            "안 is only a negator standing alone — 제안/안전 are ordinary words"
        );
    }

    #[test]
    fn polarity_of_mixed_evidence_is_unknown() {
        assert_eq!(
            claim_polarity("the old flag fails, the new one passes"),
            Polarity::Unknown
        );
    }

    #[test]
    fn verified_evidence_contradicts_the_opposite_claim_on_the_same_subject() {
        let lesson = verified_lesson(
            "workflow-verified-check-cargo-test",
            "`cargo test -p runtime` passes on this worktree",
        );
        let same_subject = page(
            "wiki/zo/gotcha-cargo-test",
            "cargo test on runtime fails without a target dir",
        );
        assert_eq!(
            judge_relation(&lesson, &same_subject),
            Some(LessonRelation::Contradicts)
        );
    }

    #[test]
    fn an_unverified_lesson_never_contradicts() {
        let mut lesson = verified_lesson(
            "gotcha-cargo-test",
            "`cargo test -p runtime` passes on this worktree",
        );
        lesson.verified = false;
        let claim = page(
            "wiki/zo/gotcha-cargo-test",
            "cargo test on runtime fails without a target dir",
        );
        assert_eq!(
            judge_relation(&lesson, &claim),
            None,
            "an unverified inference disagreeing with a person's page is noise"
        );
    }

    #[test]
    fn opposite_polarity_on_a_different_subject_is_not_a_contradiction() {
        let lesson = verified_lesson("workflow-cargo-test", "`cargo test` passes on this worktree");
        let unrelated = page(
            "wiki/zo/terminal-paste",
            "shift drag over a tracking TUI fails to select",
        );
        assert_eq!(judge_relation(&lesson, &unrelated), None);
    }

    #[test]
    fn a_claim_with_no_readable_polarity_is_never_contradicted() {
        let lesson = verified_lesson("workflow-cargo-test", "`cargo test` passes on this worktree");
        let neutral = page("wiki/zo/cargo-test", "cargo test is this worktree's gate");
        assert_eq!(judge_relation(&lesson, &neutral), None);
    }

    #[test]
    fn a_refined_signature_supersedes_the_page_it_narrows() {
        let lesson = verified_lesson(
            "workflow-verified-check-cargo-test",
            "run `cargo test` to verify",
        );
        assert_eq!(
            judge_relation(&lesson, &page("wiki/zo/workflow-verified-check", "older, broader")),
            Some(LessonRelation::Supersedes)
        );
        assert_eq!(
            judge_relation(
                &lesson,
                &page("wiki/zo/workflow-verified-checking", "a different family")
            ),
            None,
            "family membership is segment-wise, not a string prefix"
        );
        assert_eq!(
            judge_relation(
                &lesson,
                &page("wiki/zo/workflow-verified-check-cargo-test-all", "narrower")
            ),
            None,
            "the narrower page does not get superseded by the broader lesson"
        );
    }

    #[test]
    fn the_curation_pass_names_what_a_lesson_replaces() {
        let mut lesson = verified_lesson("gotcha-locale-grep", "use `LC_ALL=C grep -a`");
        lesson.replaces = vec!["gotcha-grep-silently-empty".to_string()];
        assert_eq!(
            judge_relation(
                &lesson,
                &page("wiki/zo/gotcha-grep-silently-empty", "grep returns nothing")
            ),
            Some(LessonRelation::Supersedes)
        );
    }

    #[test]
    fn cited_evidence_becomes_depends_on() {
        let targets = cited_targets(
            "follows [[wiki/zo-teammate-panes|the pane note]] and touches \
             crates/runtime/src/memory/recall.rs — see https://example.com/x/y",
        );
        assert_eq!(
            targets,
            vec![
                "wiki/zo-teammate-panes".to_string(),
                "crates/runtime/src/memory/recall.rs".to_string(),
            ],
            "an alias points at the slug, and a URL is not a vault citation"
        );

        let mut lesson = verified_lesson("gotcha-recall", "the retriever misses");
        lesson.cites = targets;
        assert_eq!(
            judge_relation(&lesson, &page("wiki/zo-teammate-panes", "panes")),
            Some(LessonRelation::DependsOn)
        );
        assert_eq!(judge_relation(&lesson, &page("wiki/unrelated", "nope")), None);
    }

    #[test]
    fn contradiction_outranks_a_mere_citation() {
        let mut lesson = verified_lesson(
            "workflow-cargo-test",
            "`cargo test -p runtime` passes on this worktree",
        );
        lesson.cites = vec!["wiki/zo/gotcha-cargo-test".to_string()];
        assert_eq!(
            judge_relation(
                &lesson,
                &page(
                    "wiki/zo/gotcha-cargo-test",
                    "cargo test on runtime fails without a target dir"
                )
            ),
            Some(LessonRelation::Contradicts),
            "the citation is how the lesson found the claim it refutes"
        );
    }

    #[test]
    fn relation_spellings_are_the_vault_frontmatter_keys() {
        assert_eq!(LessonRelation::DependsOn.as_str(), "depends_on");
        assert_eq!(LessonRelation::Supersedes.as_str(), "supersedes");
        assert_eq!(LessonRelation::Contradicts.as_str(), "contradicts");
        assert_eq!(RELATED_RELATION_KEY, "related");
    }

}
