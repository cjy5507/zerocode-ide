//! Shared deep-lane state-machine types and pure verdict readers.
//!
//! The online benchmark harness owns IO, git status, and test execution. This
//! module owns the portable plan/verifier/decision policy so runtime-facing
//! crates can reuse the same states without depending on `compat-harness`.

use crate::loop_fanout::{fold_lens_verdicts, ConsensusPolicy, LensVerdict};
use serde::Deserialize;

/// The mandatory sections a deep-lane plan must declare before any edit.
pub const REQUIRED_PLAN_SECTIONS: &[(&str, &[&str])] = &[
    (
        "files",
        &["target file", "files to change", "changed file", "file"],
    ),
    (
        "invariants",
        &["invariant", "constraint", "must not", "must stay"],
    ),
    (
        "tests",
        &["expected test", "test plan", "verification", "test"],
    ),
    ("risks", &["risk", "edge case", "pitfall", "failure mode"]),
];

/// Upper bound on a retry's failure summary, in characters.
pub const MAX_SUMMARY_CHARS: usize = 1600;

/// Verdict over a plan artifact: which required sections are present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanVerdict {
    /// True when every required section is present.
    pub valid: bool,
    /// Canonical names of sections that were not found, in declaration order.
    pub missing: Vec<String>,
}

/// How the verifier's verdict was recovered from its output.
///
/// Each variant maps to one spec parse mode via [`VerifierParse::spec_mode`].
/// [`VerifierParse::as_str`] keeps the original wire tokens so existing ledgers
/// and the shell harness stay stable across versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifierParse {
    /// A strict `{"accepted":bool,"issues":[..]}` object. Spec: `strict_valid`.
    Json,
    /// A verdict salvaged from noisy JSON or bare ACCEPT/REJECT tokens. Spec: `salvage_valid`.
    Salvaged,
    /// The verifier produced no output. Spec: `missing`.
    Empty,
    /// Output was present but carried no verdict signal. Spec: `malformed`.
    Unparseable,
    /// The verifier did not finish within its timeout. Spec: `timeout`.
    ///
    /// IO-free parsing never yields this; the harness injects it when the
    /// verifier call is killed, so it is a classified outcome, never inferred
    /// from text.
    Timeout,
    /// The verifier hit its iteration budget before emitting a verdict. Spec
    /// maps to `timeout` (same "did not finish" class), but it is a DISTINCT
    /// deep-loop policy input: a timeout is transient — retrying the attempt
    /// can plausibly verify next time — while a budget spiral is a property
    /// of the diff-vs-inspection pairing, so the same retry re-spirals and
    /// only the objective gate carries signal. Injected by the harness like
    /// `Timeout`, never inferred from text.
    BudgetExhausted,
}

impl VerifierParse {
    /// Lowercase token used on CLI/JSON boundaries. Stable across versions.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Salvaged => "salvaged",
            Self::Empty => "empty",
            Self::Unparseable => "unparseable",
            Self::Timeout => "timeout",
            Self::BudgetExhausted => "budget_exhausted",
        }
    }

    /// The spec-aligned parse-mode name used by the decision matrix and the
    /// fairness ledger: `strict_valid`, `salvage_valid`, `malformed`,
    /// `missing`, or `timeout`.
    #[must_use]
    pub const fn spec_mode(self) -> &'static str {
        match self {
            Self::Json => "strict_valid",
            Self::Salvaged => "salvage_valid",
            Self::Unparseable => "malformed",
            Self::Empty => "missing",
            // BudgetExhausted shares the spec class: the verifier did not
            // finish. The policy distinction lives in `verifier_gate_accepts`,
            // not in the fairness ledger vocabulary.
            Self::Timeout | Self::BudgetExhausted => "timeout",
        }
    }

    /// Parse from a wire token. Accepts the [`Self::as_str`] tokens and the
    /// [`Self::spec_mode`] names so a caller can pass either boundary spelling.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token.trim() {
            "json" | "strict_valid" => Self::Json,
            "salvaged" | "salvage_valid" => Self::Salvaged,
            "empty" | "missing" => Self::Empty,
            "unparseable" | "malformed" => Self::Unparseable,
            "timeout" => Self::Timeout,
            "budget_exhausted" => Self::BudgetExhausted,
            _ => return None,
        })
    }
}

/// Verdict the post-edit verifier returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifierVerdict {
    /// True only when the verifier explicitly accepted the change.
    pub accepted: bool,
    /// Concrete problems the verifier raised.
    pub issues: Vec<String>,
    /// How the verdict was recovered.
    pub parse: VerifierParse,
    /// The verifier's one-line citation of what it actually checked (files
    /// read, commands run, outputs observed). Codex-style work citation: an
    /// auditable verdict earns trust in one review instead of another
    /// verification round. `None` for older/looser verifier outputs.
    pub evidence: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifierJson {
    accepted: bool,
    #[serde(default)]
    issues: Vec<String>,
    #[serde(default)]
    evidence: Option<String>,
}

/// What the deep-lane loop should do after one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepDecision {
    /// Objective gate passed and the verifier accepted.
    Accept,
    /// Not yet good, but attempts remain.
    Retry,
    /// Not good and out of attempts.
    GiveUp,
}

/// Pure fold of one VERIFY result into the gate signal, stall signal, and next
/// loop decision. Runtime and benchmark callers own IO; this keeps the deep
/// policy (objective + verifier + progress) in one testable place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationAttempt {
    /// True when the objective/verifier combination is strong enough to accept.
    pub gate_accepted: bool,
    /// True when the same verifier issues repeated from the prior failed attempt.
    pub stalled: bool,
    /// The next action the deep loop should take.
    pub decision: DeepDecision,
}

impl DeepDecision {
    /// Lowercase token used on CLI/JSON boundaries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Retry => "retry",
            Self::GiveUp => "give_up",
        }
    }
}

#[derive(Debug)]
struct PlanSection {
    label: String,
    body: String,
}

fn section_label_and_inline_body(line: &str) -> Option<(String, Option<String>)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let is_heading = trimmed.starts_with('#');
    let body = trimmed
        .trim_start_matches(['#', '>', ' ', '\t'])
        .trim()
        .trim_matches('*')
        .trim();
    if let Some((key, inline_body)) = body.split_once(':') {
        let key = key.trim_matches('*').trim();
        if !key.is_empty() && (is_heading || is_top_level_plan_label(trimmed, key)) {
            return Some((
                key.to_ascii_lowercase(),
                Some(inline_body.trim().to_string()),
            ));
        }
    }
    if is_heading && !body.is_empty() {
        return Some((body.to_ascii_lowercase(), None));
    }
    None
}

fn is_top_level_plan_label(trimmed_line: &str, key: &str) -> bool {
    let leading = trimmed_line
        .trim_start_matches(['>', ' ', '\t'])
        .trim_start();
    let bullet_like = leading.starts_with(['-', '+', '•'])
        || (leading.starts_with('*') && !leading.starts_with("**"));
    if bullet_like || starts_with_numbered_list_marker(leading) {
        return false;
    }

    let key = key.trim().to_ascii_lowercase();
    key.len() <= 80
        && REQUIRED_PLAN_SECTIONS
            .iter()
            .any(|(_, variants)| section_matches(&key, variants))
}

fn starts_with_numbered_list_marker(line: &str) -> bool {
    let mut chars = line.chars().peekable();
    let mut saw_digit = false;
    while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
        saw_digit = true;
        chars.next();
    }
    saw_digit && matches!(chars.peek(), Some('.' | ')'))
}

fn plan_sections(markdown: &str) -> Vec<PlanSection> {
    let mut sections = Vec::new();
    let mut current: Option<PlanSection> = None;
    for line in markdown.lines() {
        if let Some((label, inline_body)) = section_label_and_inline_body(line) {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            current = Some(PlanSection {
                label,
                body: inline_body.unwrap_or_default(),
            });
            continue;
        }
        if let Some(section) = current.as_mut() {
            if !section.body.is_empty() {
                section.body.push('\n');
            }
            section.body.push_str(line);
        }
    }
    if let Some(section) = current {
        sections.push(section);
    }
    sections
}

fn section_matches(label: &str, variants: &[&str]) -> bool {
    variants.iter().any(|variant| label.contains(variant))
}

fn has_substantive_plan_body(body: &str) -> bool {
    let normalized_lines: Vec<String> = body
        .lines()
        .map(normalize_plan_body_line)
        .filter(|line| !line.is_empty())
        .collect();
    !normalized_lines.is_empty()
        && normalized_lines
            .iter()
            .any(|line| !is_placeholder_plan_body(line))
}

fn normalize_plan_body_line(line: &str) -> String {
    line.trim()
        .trim_start_matches(['-', '*', '+', '•', ' ', '\t'])
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
        .trim()
        .trim_matches(['`', '*', '_', '[', ']', '(', ')'])
        .trim()
        .to_ascii_lowercase()
}

fn is_placeholder_plan_body(line: &str) -> bool {
    let compact = line
        .chars()
        .filter(|c| !matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '-' | '—' | '_'))
        .collect::<String>()
        .trim()
        .to_string();
    matches!(
        compact.as_str(),
        "" | "todo"
            | "tbd"
            | "n/a"
            | "na"
            | "none"
            | "unknown"
            | "placeholder"
            | "fill in"
            | "to be determined"
            | "same as above"
            | "see above"
    )
}

/// Validates that a deep-lane plan declares every required section as a heading
/// or `Key:` label and supplies non-placeholder content for each one. Matching
/// is on labels, never raw prose; weak/empty sections are reported as missing
/// under their canonical section name to keep [`PlanVerdict`] stable.
#[must_use]
pub fn validate_plan(markdown: &str) -> PlanVerdict {
    let sections = plan_sections(markdown);
    let missing: Vec<String> = REQUIRED_PLAN_SECTIONS
        .iter()
        .filter(|(_, variants)| {
            !sections.iter().any(|section| {
                section_matches(&section.label, variants)
                    && has_substantive_plan_body(&section.body)
            })
        })
        .map(|(name, _)| (*name).to_string())
        .collect();
    PlanVerdict {
        valid: missing.is_empty(),
        missing,
    }
}

fn scan_json_bool(raw: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\"");
    let after = &raw[raw.find(&needle)? + needle.len()..];
    let rest = after.trim_start().strip_prefix(':')?.trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn scan_json_string_array(raw: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let Some(start) = raw.find(&needle) else {
        return Vec::new();
    };
    let bytes = raw.as_bytes();
    let mut i = start + needle.len();
    while i < bytes.len() && bytes[i] != b'[' {
        if bytes[i] == b',' || bytes[i] == b'}' {
            return Vec::new();
        }
        i += 1;
    }

    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    i += 1;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if esc {
                cur.push(match c {
                    'n' => '\n',
                    't' => '\t',
                    other => other,
                });
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
                out.push(std::mem::take(&mut cur));
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_str = true;
        } else if c == ']' {
            break;
        }
        i += 1;
    }
    out
}

/// Index of the `}` that closes the `{` at `open`, skipping any brace that sits
/// inside a JSON string. `None` if the object never balances.
fn balanced_end(raw: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &c) in raw.as_bytes().iter().enumerate().skip(open) {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// The verdict object: the balanced `{…}` that parses as a strict
/// [`VerifierJson`]. Every top-level object is tried, not just the one at the
/// first `{`, so a stray brace in the verifier's prose — a regex like `{3}`, an
/// empty `{}` — cannot mask the real verdict that follows it. The last
/// schema-matching object wins, because a model that explains before concluding
/// puts its verdict last.
fn verdict_object(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    let mut found = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = balanced_end(raw, i) {
                let candidate = &raw[i..=end];
                if serde_json::from_str::<VerifierJson>(candidate).is_ok() {
                    found = Some(candidate);
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    found
}

/// Parses a verifier's output into a [`VerifierVerdict`]. Empty, ambiguous, or
/// unparseable output never counts as acceptance.
#[must_use]
pub fn parse_verifier(raw: &str) -> VerifierVerdict {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return VerifierVerdict {
            accepted: false,
            issues: Vec::new(),
            parse: VerifierParse::Empty,
            evidence: None,
        };
    }

    if let Ok(verdict) = serde_json::from_str::<VerifierJson>(trimmed) {
        return VerifierVerdict {
            accepted: verdict.accepted,
            issues: verdict.issues,
            parse: VerifierParse::Json,
            evidence: verdict.evidence,
        };
    }

    if let Some(object) = verdict_object(trimmed) {
        if let Ok(verdict) = serde_json::from_str::<VerifierJson>(object) {
            return VerifierVerdict {
                accepted: verdict.accepted,
                issues: verdict.issues,
                parse: VerifierParse::Json,
                evidence: verdict.evidence,
            };
        }
    }

    let issues = scan_json_string_array(raw, "issues");
    if let Some(accepted) = scan_json_bool(raw, "accepted") {
        return VerifierVerdict {
            accepted,
            issues,
            parse: VerifierParse::Salvaged,
            evidence: None,
        };
    }
    let upper = raw.to_ascii_uppercase();
    if upper.contains("REJECT") {
        return VerifierVerdict {
            accepted: false,
            issues,
            parse: VerifierParse::Salvaged,
            evidence: None,
        };
    }
    if upper.contains("ACCEPT") {
        return VerifierVerdict {
            accepted: true,
            issues,
            parse: VerifierParse::Salvaged,
            evidence: None,
        };
    }

    VerifierVerdict {
        accepted: false,
        issues,
        parse: VerifierParse::Unparseable,
        evidence: None,
    }
}

/// The per-lens shape the multi-lens VERIFY prompt asks for. Each lens is an
/// independent correctness angle; a missing/null lens abstains (no signal),
/// never a silent accept. Unknown fields are tolerated (not denied): a stray key
/// like `"reasoning"` must not turn a valid all-accept response into a parse
/// failure, and the old single-`accepted` shape parses here as all-abstain and
/// then falls through to [`parse_verifier`].
#[derive(Deserialize)]
struct LensVerifierJson {
    spec: Option<bool>,
    regression: Option<bool>,
    security: Option<bool>,
    #[serde(default)]
    issues: Vec<String>,
    #[serde(default)]
    evidence: Option<String>,
}

fn lens_of(flag: Option<bool>) -> LensVerdict {
    match flag {
        Some(true) => LensVerdict::Accept,
        Some(false) => LensVerdict::Reject,
        None => LensVerdict::Abstain,
    }
}

/// A *complete* lens verdict has all three lenses present. Completeness is
/// required so a model cannot earn acceptance by OMITTING a hard lens: under
/// `AnyReject` a missing lens abstains (does not block), so an incomplete
/// response could pass without ever being judged on, e.g., security. Requiring
/// all three means an omission falls through to [`parse_verifier`] (a
/// conservative non-accept) instead — present-all to pass, never omit to pass.
fn complete_lens(lens: &LensVerifierJson) -> bool {
    lens.spec.is_some() && lens.regression.is_some() && lens.security.is_some()
}

/// Recover a *complete* per-lens object embedded in prose or a markdown code
/// fence (e.g. ```` ```json\n{…}\n``` ````): scan every balanced top-level `{…}`
/// and keep the last one whose three lenses are all present. The last match wins
/// (a model that explains before concluding puts its verdict last); requiring
/// completeness means a stray `{}` or `{ "issues": [] }` never masks the verdict.
fn last_complete_lens(raw: &str) -> Option<LensVerifierJson> {
    let bytes = raw.as_bytes();
    let mut found = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = balanced_end(raw, i) {
                if let Ok(lens) = serde_json::from_str::<LensVerifierJson>(&raw[i..=end]) {
                    if complete_lens(&lens) {
                        found = Some(lens);
                    }
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    found
}

/// Parse a multi-lens VERIFY response into a single [`VerifierVerdict`].
///
/// The change is judged by three independent correctness lenses (spec /
/// regression / security) in one sub-turn; their verdicts are folded by
/// [`fold_lens_verdicts`] under [`ConsensusPolicy::AnyReject`] — a single
/// credible objection blocks acceptance, matching the deep lane's strict,
/// anti-optimistic stance. The folded `Option<bool>` maps to `accepted`; the
/// `issues` carry every lens's concrete objection.
///
/// Only a *complete* lens response (all three present) uses the fold; the lens
/// object is recovered even when fenced or prose-wrapped (`last_complete_lens`).
/// Anything else — a non-lens response, an incomplete one (a lens omitted), or an
/// unusable one — falls back to [`parse_verifier`], so the old single-`accepted`
/// contract and the salvage/keyword heuristics still apply and an unusable or
/// lens-omitting response resolves to a conservative non-accept (never a silent
/// accept).
#[must_use]
pub fn parse_lens_verifier(raw: &str) -> VerifierVerdict {
    let trimmed = raw.trim();
    let lens = serde_json::from_str::<LensVerifierJson>(trimmed)
        .ok()
        .filter(complete_lens)
        .or_else(|| last_complete_lens(trimmed));
    if let Some(lens) = lens {
        let verdicts = [
            lens_of(lens.spec),
            lens_of(lens.regression),
            lens_of(lens.security),
        ];
        // All three present ⇒ no abstains ⇒ fold is always `Some`.
        if let Some(accepted) = fold_lens_verdicts(&verdicts, ConsensusPolicy::AnyReject) {
            return VerifierVerdict {
                accepted,
                issues: lens.issues,
                parse: VerifierParse::Json,
                evidence: lens.evidence,
            };
        }
    }
    parse_verifier(raw)
}

/// Deep-lane control decision after one attempt. `attempt` is 1-based.
#[must_use]
pub const fn decide(
    attempt: u32,
    max_attempts: u32,
    objective_ok: bool,
    verifier_accepted: bool,
) -> DeepDecision {
    if objective_ok && verifier_accepted {
        DeepDecision::Accept
    } else if attempt < max_attempts {
        DeepDecision::Retry
    } else {
        DeepDecision::GiveUp
    }
}

/// Deep-lane decision that also honors the ALP "loop makes no more progress"
/// stop condition (doc §3): when an attempt fails for the **same reason** as the
/// one before it, retrying again is wasted budget, so give up early even with
/// attempts to spare.
///
/// `stalled` means "this failure is identical to the previous attempt's failure"
/// (see [`failures_match`]) — the caller owns comparing the two verdicts and
/// passes the boolean so this stays a pure, `const`, exhaustively testable
/// decision. A passing attempt always [`Accept`](DeepDecision::Accept)s; an
/// unresolved stall short-circuits to [`GiveUp`](DeepDecision::GiveUp);
/// otherwise this matches [`decide`] exactly (retry while attempts remain).
///
/// The first failure is never a stall — there is nothing to repeat yet — so the
/// caller passes `stalled = false` on attempt 1.
#[must_use]
pub const fn decide_with_progress(
    attempt: u32,
    max_attempts: u32,
    objective_ok: bool,
    verifier_accepted: bool,
    stalled: bool,
) -> DeepDecision {
    if objective_ok && verifier_accepted {
        DeepDecision::Accept
    } else if stalled {
        // No progress since the last attempt: repeating the same repair would
        // burn tokens for the same red result. Stop honestly now.
        DeepDecision::GiveUp
    } else if attempt < max_attempts {
        DeepDecision::Retry
    } else {
        DeepDecision::GiveUp
    }
}

/// Whether two verifier failures are the *same* failure — the signal that the
/// loop has stalled (doc §3). Compares the concrete issues the verifier raised,
/// order- and whitespace-insensitively, so cosmetic reordering or reformatting
/// of an otherwise identical complaint still counts as no progress.
///
/// Two empty issue lists are treated as **not** matching: an empty verdict
/// carries no evidence of repetition, so the loop should keep its normal
/// attempt budget rather than give up on a content-free signal.
#[must_use]
pub fn failures_match(previous: &[String], current: &[String]) -> bool {
    if previous.is_empty() || current.is_empty() {
        return false;
    }
    normalized_issue_set(previous) == normalized_issue_set(current)
}

/// Normalize an issue list into an order-independent set of comparable keys:
/// trimmed, internal whitespace collapsed, lowercased, blanks dropped.
fn normalized_issue_set(issues: &[String]) -> std::collections::BTreeSet<String> {
    issues
        .iter()
        .map(|issue| {
            issue
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase()
        })
        .filter(|issue| !issue.is_empty())
        .collect()
}

/// Whether a verifier verdict is strong enough to satisfy the verifier gate.
///
/// Strict JSON acceptances always pass. A rejection needs to state SOME reason
/// to veto an objectively green attempt; a verdict that rejects while naming
/// nothing at all is a malformed verdict, not a finding an implementer could
/// act on — and forcing another edit pass against it is how a green change gets
/// ground into a zero-progress retry loop. Salvaged rejections remain
/// non-blocking when the objective is green. Empty output and unparseable
/// output still block acceptance.
///
/// `has_reasons` deliberately spans BOTH `issues` and `evidence`: a verifier
/// that writes its objection into the wrong field has still objected, and
/// dropping that would trade a retry loop for silently discarding a real
/// finding. Only a verdict that is reason-less everywhere gets relaxed.
///
/// Scope worth naming: `objective_ok` is not always a positive observation. A
/// session with no configured check command reports green vacuously, and a run
/// whose baseline was already red is treated as green so pre-existing breakage
/// cannot pin the lane. In those two cases this relaxation rests on nothing
/// having *contradicted* the change rather than on evidence it is sound.
#[must_use]
pub const fn verifier_gate_accepts(
    objective_ok: bool,
    verifier_accepted: bool,
    parse: VerifierParse,
    has_reasons: bool,
) -> bool {
    if verifier_accepted {
        true
    } else {
        objective_ok
            && (matches!(parse, VerifierParse::Salvaged)
                || (matches!(parse, VerifierParse::Json) && !has_reasons)
                // An inspection that ran past its iteration budget produced
                // no verdict AND named no issue — blocking on it would spend
                // a full retry to re-run the same spiral. The objective gate
                // (tests) stays the hard gate; a budget stop with any named
                // reason still blocks.
                || (matches!(parse, VerifierParse::BudgetExhausted) && !has_reasons))
    }
}

/// Fold one verifier output into the deep-loop state transition. This is the
/// single-responsibility seam used by live runtime and harness callers: they pass
/// observed facts, and this pure helper returns the policy outcome.
#[must_use]
pub fn fold_verification_attempt(
    attempt: u32,
    max_attempts: u32,
    objective_ok: bool,
    verifier: &VerifierVerdict,
    previous_issues: &[String],
) -> VerificationAttempt {
    // A reason can land in either field: `issues` is where the contract asks
    // for it, but a verifier that explained itself in `evidence` has still
    // named something, and waving that through would discard a real finding.
    let has_reasons = !verifier.issues.is_empty()
        || verifier
            .evidence
            .as_deref()
            .is_some_and(|evidence| !evidence.trim().is_empty());
    let gate_accepted =
        verifier_gate_accepts(objective_ok, verifier.accepted, verifier.parse, has_reasons);
    let stalled = !gate_accepted && failures_match(previous_issues, &verifier.issues);
    let decision =
        decide_with_progress(attempt, max_attempts, objective_ok, gate_accepted, stalled);
    VerificationAttempt {
        gate_accepted,
        stalled,
        decision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_with_all_sections_is_valid() {
        let plan = "\
# Plan
## Target files
- src/model.js
## Invariants
- public API unchanged
## Expected tests
- node --test
## Risks
- serializer drift";
        let verdict = validate_plan(plan);
        assert!(verdict.valid, "missing: {:?}", verdict.missing);
        assert!(verdict.missing.is_empty());
    }

    #[test]
    fn plan_accepts_key_colon_labels() {
        let plan =
            "Files: a.js, b.js\nInvariants: none break\nTests: cargo test\nRisks: edge cases";
        assert!(validate_plan(plan).valid);
    }

    #[test]
    fn plan_body_colons_do_not_start_fake_sections() {
        let plan = "\
## Target files
- **src/model.js** — change `createMoney(amount)` → `createMoney(amount, currency)`.
## Invariants
- `module.exports` names unchanged in all files.
## Expected tests
1. *creates immutable money with an explicit currency* — object has `amount` and `currency`.
2. *serializes and deserializes currency end-to-end* — exact JSON string order matters.
## Risks
- **Literal message drift:** the string must be exactly `currency must be a three-letter uppercase code`.
- **Serialize key order:** test compares an exact JSON string.";
        let verdict = validate_plan(plan);
        assert!(verdict.valid, "missing: {:?}", verdict.missing);
    }

    #[test]
    fn plan_accepts_bold_labels() {
        let plan = "**Changed files**: x\n**Constraint**: y\n**Verification**: z\n**Edge case**: w";
        assert!(validate_plan(plan).valid);
    }

    #[test]
    fn plan_missing_sections_reported_in_order() {
        let plan = "## Target files\n- a.js\n## Risks\n- none";
        let verdict = validate_plan(plan);
        assert!(!verdict.valid);
        assert_eq!(
            verdict.missing,
            vec![
                "invariants".to_string(),
                "tests".to_string(),
                "risks".to_string()
            ]
        );
    }

    #[test]
    fn plan_rejects_empty_or_placeholder_sections() {
        let plan = "\
## Target files
TODO
## Invariants
- TBD
## Expected tests
- cargo test
## Risks
- same as above";
        let verdict = validate_plan(plan);
        assert!(!verdict.valid);
        assert_eq!(
            verdict.missing,
            vec![
                "files".to_string(),
                "invariants".to_string(),
                "risks".to_string()
            ]
        );
    }

    #[test]
    fn plan_accepts_concise_non_placeholder_section_bodies() {
        let plan = "Files: x\nInvariants: y\nTests: z\nRisks: w";
        assert!(validate_plan(plan).valid);
    }

    #[test]
    fn plan_rejects_inline_placeholder_labels() {
        let plan = "Files: todo\nInvariants: keep api\nTests: cargo test\nRisks: n/a";
        let verdict = validate_plan(plan);
        assert!(!verdict.valid);
        assert_eq!(
            verdict.missing,
            vec!["files".to_string(), "risks".to_string()]
        );
    }

    #[test]
    fn plan_prose_mention_does_not_satisfy_section() {
        let plan =
            "## Files\n- a.js\n## Invariants\n- keep it\n## Risks\n- we should test carefully";
        let verdict = validate_plan(plan);
        assert!(!verdict.valid);
        assert_eq!(verdict.missing, vec!["tests".to_string()]);
    }

    #[test]
    fn verifier_accepts_explicit_true() {
        let verdict = parse_verifier(r#"{"accepted": true, "issues": []}"#);
        assert!(verdict.accepted);
        assert!(verdict.issues.is_empty());
    }

    #[test]
    fn verifier_rejects_with_issues() {
        let verdict = parse_verifier(
            r#"{"accepted": false, "issues": ["missing null check", "edge case off-by-one"]}"#,
        );
        assert!(!verdict.accepted);
        assert_eq!(verdict.issues.len(), 2);
        assert_eq!(verdict.issues[1], "edge case off-by-one");
    }

    #[test]
    fn verifier_field_wins_over_prose() {
        let verdict = parse_verifier(
            r#"Looks good, I would ACCEPT. {"accepted": false, "issues": ["actually broken"]}"#,
        );
        assert!(!verdict.accepted);
    }

    #[test]
    fn verifier_token_fallback_reject_beats_accept() {
        let verdict =
            parse_verifier("Verdict: REJECT - would otherwise ACCEPT but a test regressed.");
        assert!(!verdict.accepted);
    }

    #[test]
    fn verifier_unescapes_array_strings() {
        let verdict =
            parse_verifier(r#"{"accepted": false, "issues": ["line1\nline2", "tab\tsep"]}"#);
        assert_eq!(verdict.issues[0], "line1\nline2");
        assert_eq!(verdict.issues[1], "tab\tsep");
    }

    #[test]
    fn verifier_clean_json_is_classified_json() {
        assert_eq!(
            parse_verifier(r#"{"accepted": true, "issues": []}"#).parse,
            VerifierParse::Json
        );
        assert_eq!(
            parse_verifier(r#"{"accepted": true}"#).parse,
            VerifierParse::Json
        );
    }

    #[test]
    fn verifier_unknown_fields_fall_out_of_strict_but_salvage() {
        let verdict = parse_verifier(r#"{"accepted": true, "issues": [], "confidence": "high"}"#);
        assert!(verdict.accepted);
        assert_eq!(verdict.parse, VerifierParse::Salvaged);
    }

    #[test]
    fn verifier_prose_wrapped_clean_json_is_json() {
        let verdict = parse_verifier(r#"Verdict below. {"accepted": false, "issues": ["x"]}"#);
        assert!(!verdict.accepted);
        assert_eq!(verdict.issues, vec!["x".to_string()]);
        assert_eq!(verdict.parse, VerifierParse::Json);
    }

    #[test]
    fn verifier_brace_in_prose_before_json_is_classified_json() {
        // Real regression from `coding-loop-token-...-153415`: two deep cells were
        // wrongly salvaged because a stray brace in the verifier's prose preceded
        // the clean verdict, so the old first-brace scan grabbed the fragment. A
        // schema-aware scan must classify both as strict Json.

        // deep-schema-propagation: a regex `{3}` appears before the verdict.
        let schema = "Verified: the `currency` field is validated (regex `^[A-Z]{3}$`) \
                      and propagated through serialize/deserialize.\n\n\
                      {\"accepted\": true, \"issues\": []}";
        let v = parse_verifier(schema);
        assert!(v.accepted, "schema-propagation verdict must be accepted");
        assert_eq!(v.parse, VerifierParse::Json);

        // deep-wide-rename: an empty `{}` appears before the verdict.
        let rename = "The rename is complete and consistent; `opts` defaults to `{}` \
                      so callers that don't pass options still work.\n\n\
                      {\"accepted\": true, \"issues\": []}";
        let v = parse_verifier(rename);
        assert!(v.accepted, "wide-rename verdict must be accepted");
        assert_eq!(v.parse, VerifierParse::Json);
    }

    #[test]
    fn verifier_fenced_json_is_json() {
        let verdict = parse_verifier("```json\n{\"accepted\": true, \"issues\": []}\n```");
        assert!(verdict.accepted);
        assert_eq!(verdict.parse, VerifierParse::Json);
    }

    #[test]
    fn verdict_object_picks_schema_match_not_first_brace() {
        // Stray braces — a regex-like `{3}` and an empty `{}` — precede the real
        // verdict. The scan must skip them and return the schema-matching object.
        assert_eq!(
            verdict_object("note {} and {3} then {\"accepted\": false, \"issues\": [\"x\"]}"),
            Some(r#"{"accepted": false, "issues": ["x"]}"#)
        );
        // No verdict-shaped object anywhere.
        assert_eq!(verdict_object("just {} and {3}"), None);
        assert_eq!(verdict_object("no object here"), None);
    }

    #[test]
    fn verdict_object_ignores_braces_inside_strings() {
        // A `}` inside a JSON string must not close the object early.
        assert_eq!(
            verdict_object(r#"{"accepted": true, "issues": ["a}b"]}"#),
            Some(r#"{"accepted": true, "issues": ["a}b"]}"#)
        );
    }

    #[test]
    fn verifier_empty_output_is_empty_parse() {
        let verdict = parse_verifier("   \n  ");
        assert!(!verdict.accepted);
        assert_eq!(verdict.parse, VerifierParse::Empty);
    }

    #[test]
    fn verifier_no_signal_is_unparseable() {
        let verdict = parse_verifier("hmm, not sure, the diff is large");
        assert!(!verdict.accepted);
        assert_eq!(verdict.parse, VerifierParse::Unparseable);
    }

    #[test]
    fn lens_verifier_all_accept_accepts() {
        let verdict =
            parse_lens_verifier(r#"{"spec": true, "regression": true, "security": true, "issues": []}"#);
        assert!(verdict.accepted);
        assert_eq!(verdict.parse, VerifierParse::Json);
        assert!(verdict.issues.is_empty());
    }

    #[test]
    fn lens_verifier_any_reject_blocks() {
        // AnyReject: a single lens objection rejects the whole change, even when
        // the other lenses accept.
        let verdict = parse_lens_verifier(
            r#"{"spec": true, "regression": false, "security": true, "issues": ["deleted a test"]}"#,
        );
        assert!(!verdict.accepted);
        assert_eq!(verdict.parse, VerifierParse::Json);
        assert_eq!(verdict.issues, vec!["deleted a test".to_string()]);
    }

    #[test]
    fn lens_rejection_without_issues_does_not_retry_a_green_attempt() {
        let verdict = parse_lens_verifier(
            r#"{"spec": false, "regression": true, "security": true, "issues": []}"#,
        );
        let folded = fold_verification_attempt(1, 2, true, &verdict, &[]);

        assert_eq!(folded.decision, DeepDecision::Accept);
    }

    /// Omitting `issues` entirely parses to the same empty vec through serde's
    /// default, so it must reach the same decision as spelling out `[]`.
    #[test]
    fn lens_rejection_omitting_the_issues_field_is_treated_the_same() {
        let verdict =
            parse_lens_verifier(r#"{"spec": false, "regression": true, "security": true}"#);
        let folded = fold_verification_attempt(1, 2, true, &verdict, &[]);

        assert_eq!(folded.decision, DeepDecision::Accept);
    }

    /// A rejection that states its objection in `evidence` rather than `issues`
    /// is a real finding, not the reason-less malformed verdict the relaxation
    /// exists for. Treating it as malformed would silently discard what the
    /// verifier actually caught — the exact failure the relaxation must not
    /// trade for.
    #[test]
    fn lens_rejection_reasoned_only_in_evidence_still_blocks() {
        let verdict = parse_lens_verifier(
            r#"{"spec": false, "regression": true, "security": true, "issues": [], "evidence": "spec requires the retry cap to apply per host, but the diff applies it globally"}"#,
        );
        let folded = fold_verification_attempt(1, 2, true, &verdict, &[]);

        assert_eq!(folded.decision, DeepDecision::Retry);
    }

    /// The relaxation is scoped to an objectively green attempt. With the
    /// objective check red there is nothing vouching for the change, so a
    /// reason-less rejection must not be waved through.
    #[test]
    fn reason_less_rejection_still_blocks_when_the_objective_check_is_red() {
        assert!(!verifier_gate_accepts(
            false,
            false,
            VerifierParse::Json,
            false,
        ));
        assert!(!verifier_gate_accepts(
            false,
            false,
            VerifierParse::Salvaged,
            false,
        ));
    }

    #[test]
    fn lens_verifier_incomplete_response_falls_back_not_accepts() {
        // An incomplete lens response (a lens omitted) must NOT pass — otherwise a
        // model could earn acceptance by omitting the hard lens it would fail.
        // Requiring all three present means an omission falls back to the
        // conservative single-verdict reader (non-accept here).
        let security_omitted = parse_lens_verifier(r#"{"spec": true, "regression": true, "issues": []}"#);
        assert!(
            !security_omitted.accepted,
            "omitting a lens must not accept (no omit-to-pass incentive)"
        );
        // A present reject still blocks regardless of completeness handling.
        let one_reject =
            parse_lens_verifier(r#"{"spec": true, "security": false, "issues": ["x"]}"#);
        assert!(!one_reject.accepted);
    }

    #[test]
    fn lens_verifier_fenced_or_wrapped_json_is_recovered() {
        // Models sometimes wrap the verdict in a markdown fence or prose despite
        // the prompt; a COMPLETE lens object must still be recovered, not silently
        // rejected. Regression for the adversarial-review finding.
        let fenced = parse_lens_verifier(
            "```json\n{\"spec\": true, \"regression\": true, \"security\": true, \"issues\": []}\n```",
        );
        assert!(fenced.accepted, "fenced complete lens JSON must be recovered");
        assert_eq!(fenced.parse, VerifierParse::Json);

        let prose = parse_lens_verifier(
            "Here is my verdict:\n{\"spec\": false, \"regression\": true, \"security\": true, \"issues\": [\"missing test\"]}",
        );
        assert!(!prose.accepted);
        assert_eq!(prose.issues, vec!["missing test".to_string()]);
    }

    #[test]
    fn lens_verifier_all_abstain_falls_back_to_conservative_non_accept() {
        // No lens signal at all → fall back to parse_verifier, which never treats
        // an empty/ambiguous response as acceptance.
        let empty_obj = parse_lens_verifier(r#"{"issues": []}"#);
        assert!(!empty_obj.accepted);
    }

    #[test]
    fn lens_verifier_falls_back_to_single_accepted_contract() {
        // The old {accepted, issues} shape (a model ignoring the rubric) still
        // resolves via the fallback to parse_verifier.
        let accept = parse_lens_verifier(r#"{"accepted": true, "issues": []}"#);
        assert!(accept.accepted);
        let reject = parse_lens_verifier(r#"{"accepted": false, "issues": ["bad"]}"#);
        assert!(!reject.accepted);
    }

    #[test]
    fn decide_accepts_when_both_gates_pass() {
        assert_eq!(decide(1, 3, true, true), DeepDecision::Accept);
    }

    #[test]
    fn decide_retries_with_attempts_left() {
        assert_eq!(decide(1, 3, false, false), DeepDecision::Retry);
        assert_eq!(decide(1, 3, true, false), DeepDecision::Retry);
        assert_eq!(decide(1, 3, false, true), DeepDecision::Retry);
    }

    #[test]
    fn decide_gives_up_when_out_of_attempts() {
        assert_eq!(decide(3, 3, false, true), DeepDecision::GiveUp);
        assert_eq!(decide(2, 2, true, false), DeepDecision::GiveUp);
    }

    #[test]
    fn decide_accepts_on_last_attempt_if_good() {
        assert_eq!(decide(3, 3, true, true), DeepDecision::Accept);
    }

    #[test]
    fn decide_with_progress_matches_decide_when_not_stalled() {
        // With `stalled = false` the new decision is identical to `decide`.
        for attempt in 1..=3 {
            for max in 1..=3 {
                for objective_ok in [false, true] {
                    for accepted in [false, true] {
                        assert_eq!(
                            decide_with_progress(attempt, max, objective_ok, accepted, false),
                            decide(attempt, max, objective_ok, accepted),
                            "attempt={attempt} max={max} obj={objective_ok} acc={accepted}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn decide_with_progress_gives_up_early_on_a_stall() {
        // Attempts remain (2 of 5), but the same failure repeated → stop now.
        assert_eq!(
            decide_with_progress(2, 5, false, false, true),
            DeepDecision::GiveUp
        );
    }

    #[test]
    fn decide_with_progress_accepts_even_if_flagged_stalled() {
        // A green+accepted attempt wins regardless of the stall flag: success
        // is never overridden by the no-progress brake.
        assert_eq!(
            decide_with_progress(2, 5, true, true, true),
            DeepDecision::Accept
        );
    }

    #[test]
    fn failures_match_is_order_and_whitespace_insensitive() {
        let a = vec!["Test  foo FAILED".to_string(), "lint: bar".to_string()];
        let b = vec!["lint:   bar".to_string(), "test foo failed".to_string()];
        assert!(failures_match(&a, &b));
    }

    #[test]
    fn failures_match_distinguishes_different_failures() {
        let a = vec!["test foo failed".to_string()];
        let b = vec!["test baz failed".to_string()];
        assert!(!failures_match(&a, &b));
    }

    #[test]
    fn failures_match_treats_empty_verdicts_as_no_match() {
        // An empty verdict is no evidence of repetition — keep the normal budget.
        assert!(!failures_match(&[], &["x".to_string()]));
        assert!(!failures_match(&["x".to_string()], &[]));
        assert!(!failures_match(&[], &[]));
    }

    #[test]
    fn budget_exhausted_verifier_defers_to_the_objective_gate() {
        let unfinished = VerifierVerdict {
            accepted: false,
            issues: Vec::new(),
            parse: VerifierParse::BudgetExhausted,
            evidence: None,
        };
        // Objective green: the unfinished OPTIONAL inspection must not spend
        // a retry — the same spiral would just run again.
        let green = fold_verification_attempt(1, 3, true, &unfinished, &[]);
        assert!(green.gate_accepted);
        assert_eq!(green.decision, DeepDecision::Accept);
        // Objective red stays blocking regardless of the verifier.
        let red = fold_verification_attempt(1, 3, false, &unfinished, &[]);
        assert!(!red.gate_accepted);
        assert_eq!(red.decision, DeepDecision::Retry);
        // A named reason blocks even at budget exhaustion.
        let with_issue = VerifierVerdict {
            issues: vec!["missed spec".to_string()],
            ..unfinished.clone()
        };
        assert!(!fold_verification_attempt(1, 3, true, &with_issue, &[]).gate_accepted);
        // Timeout (transient — a retry can plausibly verify) must NOT merge
        // into this class: it still withholds the gate under objective green.
        let timeout = VerifierVerdict {
            parse: VerifierParse::Timeout,
            ..unfinished
        };
        assert!(!fold_verification_attempt(1, 3, true, &timeout, &[]).gate_accepted);
    }

    #[test]
    fn fold_verification_attempt_accepts_strict_green_verdict() {
        let verifier = VerifierVerdict {
            accepted: true,
            issues: Vec::new(),
            parse: VerifierParse::Json,
            evidence: None,
        };
        let folded = fold_verification_attempt(1, 3, true, &verifier, &[]);

        assert!(folded.gate_accepted);
        assert!(!folded.stalled);
        assert_eq!(folded.decision, DeepDecision::Accept);
    }

    #[test]
    fn fold_verification_attempt_gives_up_on_repeated_issues() {
        let prior = vec!["Missed call site".to_string()];
        let verifier = VerifierVerdict {
            accepted: false,
            issues: vec![" missed   call site ".to_string()],
            parse: VerifierParse::Json,
            evidence: None,
        };
        let folded = fold_verification_attempt(2, 4, true, &verifier, &prior);

        assert!(!folded.gate_accepted);
        assert!(folded.stalled);
        assert_eq!(folded.decision, DeepDecision::GiveUp);
    }

    #[test]
    fn fold_verification_attempt_preserves_salvaged_green_acceptance() {
        let verifier = VerifierVerdict {
            accepted: false,
            issues: vec!["non-json rejection should be salvage-only".to_string()],
            parse: VerifierParse::Salvaged,
            evidence: None,
        };
        let folded = fold_verification_attempt(1, 3, true, &verifier, &[]);

        assert!(folded.gate_accepted);
        assert!(!folded.stalled);
        assert_eq!(folded.decision, DeepDecision::Accept);
    }

    #[test]
    fn verifier_gate_accepts_salvaged_rejection_only_when_objective_is_green() {
        assert!(verifier_gate_accepts(
            true,
            false,
            VerifierParse::Salvaged,
            true,
        ));
        assert!(!verifier_gate_accepts(
            false,
            false,
            VerifierParse::Salvaged,
            true,
        ));
    }

    #[test]
    fn verifier_gate_keeps_itemized_strict_json_rejection_blocking() {
        assert!(!verifier_gate_accepts(true, false, VerifierParse::Json, true));
        assert!(verifier_gate_accepts(false, true, VerifierParse::Json, false));
    }
}
